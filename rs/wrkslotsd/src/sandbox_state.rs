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

pub(crate) const SETTING_KEYS: [&str; 14] = [
    "isolation",
    "home",
    "home_shared",
    "home_private",
    "home_private_files",
    "home_hidden",
    "home_expose",
    "outputs",
    "read_write",
    "env",
    "tmp_size",
    "protect_system",
    "coordinator_writable",
    "limits",
];

const RENAMED_KEYS: [(&str, &str); 9] = [
    ("home_writable", "home_private"),
    ("home_paths", "home_expose (with home: hidden)"),
    ("memory_max", "limits.memory_max"),
    ("memory_high", "limits.memory_high"),
    ("cpu_quota", "limits.cpu_quota"),
    ("tasks_max", "limits.tasks_max"),
    ("io_weight", "limits.io_weight"),
    ("all_slots_memory_max", "limits.all_slots_memory_max"),
    ("all_slots_cpu_quota", "limits.all_slots_cpu_quota"),
];

/// The full `configuration.sandbox` section, mirroring Python
/// `SandboxSettings` with its defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SandboxSettings {
    pub(crate) isolation: String,
    pub(crate) home: String,
    pub(crate) home_shared: Vec<String>,
    pub(crate) home_private: Vec<String>,
    pub(crate) home_private_files: Vec<String>,
    pub(crate) home_hidden: Vec<String>,
    pub(crate) home_expose: Vec<String>,
    pub(crate) outputs: Vec<String>,
    pub(crate) read_write: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) tmp_size: String,
    pub(crate) protect_system: bool,
    pub(crate) coordinator_writable: String,
    pub(crate) limits: SandboxLimits,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        let strings = |items: &[&str]| items.iter().map(|item| item.to_string()).collect();
        Self {
            isolation: "userns".to_string(),
            home: "ro".to_string(),
            home_shared: strings(&[
                ".claude",
                ".codex",
                ".muse",
                ".config/muse",
                ".config/opencode",
                ".local/share/opencode",
                ".local/state/herdr",
                ".local/share/muse",
                ".cargo/registry",
                ".cargo/git",
            ]),
            home_private: strings(&[".cache", ".buck"]),
            home_private_files: strings(&[".claude.json"]),
            home_hidden: strings(&[
                ".ssh",
                ".gnupg",
                ".aws",
                ".azure",
                ".kube",
                ".docker/config.json",
                ".netrc",
                ".git-credentials",
                ".pgpass",
                ".arcrc",
                ".config/gh/hosts.yml",
                ".config/gcloud",
            ]),
            home_expose: strings(&["bin", ".local/bin"]),
            outputs: strings(&["ai_docs", "experiments"]),
            read_write: Vec::new(),
            env: Vec::new(),
            tmp_size: "16G".to_string(),
            protect_system: true,
            coordinator_writable: "worktrees".to_string(),
            limits: SandboxLimits::default(),
        }
    }
}

fn str_tuple(value: Option<&Value>, label: &str) -> Result<Vec<String>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err(format!("{label} must be a list of strings"));
                };
                out.push(text.to_string());
            }
            Ok(out)
        }
        _ => Err(format!("{label} must be a list of strings")),
    }
}

fn relative_ok(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && value != "."
        && !value.split('/').any(|part| part == "..")
}

fn relative_list(
    map: &serde_json::Map<String, Value>,
    key: &str,
    default: &[String],
) -> Result<Vec<String>, String> {
    let label = format!("configuration.sandbox.{key}");
    let values = match map.get(key) {
        None => default.to_vec(),
        other => str_tuple(other, &label)?,
    };
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !relative_ok(&value) {
            return Err(format!(
                "{label} entries must be relative paths without '..': '{value}'"
            ));
        }
        if !out.contains(&value) {
            out.push(value);
        }
    }
    Ok(out)
}

fn tmp_size_ok(value: &str) -> bool {
    // Python `_TMP_SIZE_RE`: ^(\d+[KMGT]?|\d+%)$
    let number = value
        .strip_suffix('%')
        .unwrap_or_else(|| value.strip_suffix(['K', 'M', 'G', 'T']).unwrap_or(value));
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Validate a `configuration.sandbox` mapping, mirroring Python
/// `settings_from_obj` with its error strings.
pub(crate) fn settings_from_obj(raw: &Value) -> Result<SandboxSettings, String> {
    let Some(map) = raw.as_object() else {
        return Err("configuration.sandbox must be an object".to_string());
    };
    let mut unknown: Vec<&String> = map
        .keys()
        .filter(|key| !SETTING_KEYS.contains(&key.as_str()))
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        let hints: Vec<String> = unknown
            .iter()
            .map(
                |key| match RENAMED_KEYS.iter().find(|(old, _)| *old == key.as_str()) {
                    Some((_, new)) => format!("{key} (now {new})"),
                    None => key.to_string(),
                },
            )
            .collect();
        return Err(format!(
            "configuration.sandbox has unknown keys: {}",
            hints.join(", ")
        ));
    }
    let defaults = SandboxSettings::default();
    let isolation = map
        .get("isolation")
        .and_then(Value::as_str)
        .unwrap_or(&defaults.isolation);
    if !ISOLATIONS.contains(&isolation) {
        let hint = if isolation == "namespace" {
            " (namespace was renamed userns)"
        } else {
            ""
        };
        return Err(format!(
            "configuration.sandbox.isolation must be one of {}{hint}",
            ISOLATIONS.join(", ")
        ));
    }
    let home = map
        .get("home")
        .and_then(Value::as_str)
        .unwrap_or(&defaults.home);
    if !HOME_MODES.contains(&home) {
        let hint = if matches!(home, "select" | "none") {
            " (use hidden with home_expose)"
        } else {
            ""
        };
        return Err(format!(
            "configuration.sandbox.home must be one of {}{hint}",
            HOME_MODES.join(", ")
        ));
    }
    let protect = match map.get("protect_system") {
        None => defaults.protect_system,
        Some(Value::Bool(flag)) => *flag,
        _ => return Err("configuration.sandbox.protect_system must be true or false".to_string()),
    };
    let tmp_size = map
        .get("tmp_size")
        .and_then(Value::as_str)
        .unwrap_or(&defaults.tmp_size);
    if !tmp_size_ok(tmp_size) {
        return Err(format!(
            "configuration.sandbox.tmp_size must look like 16G or 50%, not '{tmp_size}'"
        ));
    }
    let scope = map
        .get("coordinator_writable")
        .and_then(Value::as_str)
        .unwrap_or(&defaults.coordinator_writable);
    if !COORDINATOR_SCOPES.contains(&scope) {
        return Err(format!(
            "configuration.sandbox.coordinator_writable must be one of {}",
            COORDINATOR_SCOPES.join(", ")
        ));
    }
    let mut env = defaults.env.clone();
    if let Some(raw_env) = map.get("env") {
        let Some(env_map) = raw_env.as_object() else {
            return Err(
                "configuration.sandbox.env must be an object of NAME: value strings".to_string(),
            );
        };
        env = Vec::new();
        for (key, item) in env_map {
            let valid_name = !key.is_empty()
                && key.bytes().enumerate().all(|(index, byte)| {
                    byte.is_ascii_alphabetic()
                        || byte == b'_'
                        || (index > 0 && byte.is_ascii_digit())
                });
            if !valid_name {
                return Err(format!(
                    "configuration.sandbox.env has an invalid variable name: '{key}'"
                ));
            }
            let Some(text) = item.as_str() else {
                return Err(format!(
                    "configuration.sandbox.env.{key} must be a one-line string"
                ));
            };
            if text.contains('\n') || text.contains('\0') {
                return Err(format!(
                    "configuration.sandbox.env.{key} must be a one-line string"
                ));
            }
            env.push((key.clone(), text.to_string()));
        }
    }
    let limits = match map.get("limits") {
        None => defaults.limits.clone(),
        Some(raw_limits) => limits_from_obj(raw_limits)?,
    };
    let read_write_label = "configuration.sandbox.read_write";
    let mut read_write: Vec<String> = Vec::new();
    for item in str_tuple(map.get("read_write"), read_write_label)? {
        if !(item.starts_with('/') || item == "~" || item.starts_with("~/")) {
            return Err(format!(
                "{read_write_label} entries must be absolute or ~/-relative paths: '{item}'"
            ));
        }
        if !read_write.contains(&item) {
            read_write.push(item);
        }
    }
    Ok(SandboxSettings {
        isolation: isolation.to_string(),
        home: home.to_string(),
        home_shared: relative_list(map, "home_shared", &defaults.home_shared)?,
        home_private: relative_list(map, "home_private", &defaults.home_private)?,
        home_private_files: {
            let values = relative_list(map, "home_private_files", &defaults.home_private_files)?;
            for value in &values {
                if value.contains('/') {
                    return Err(format!(
                        "configuration.sandbox.home_private_files entries must be top-level $HOME names: '{value}'"
                    ));
                }
            }
            values
        },
        home_hidden: relative_list(map, "home_hidden", &defaults.home_hidden)?,
        home_expose: relative_list(map, "home_expose", &defaults.home_expose)?,
        outputs: relative_list(map, "outputs", &defaults.outputs)?,
        read_write,
        env,
        tmp_size: tmp_size.to_string(),
        protect_system: protect,
        coordinator_writable: scope.to_string(),
        limits,
    })
}

/// The full JSON form of the settings, mirroring Python
/// `settings_to_obj` (every key spelled out).
pub(crate) fn settings_to_obj(settings: &SandboxSettings) -> Value {
    json!({
        "isolation": settings.isolation,
        "home": settings.home,
        "home_shared": settings.home_shared,
        "home_private": settings.home_private,
        "home_private_files": settings.home_private_files,
        "home_hidden": settings.home_hidden,
        "home_expose": settings.home_expose,
        "outputs": settings.outputs,
        "read_write": settings.read_write,
        "env": settings
            .env
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect::<serde_json::Map<String, Value>>(),
        "tmp_size": settings.tmp_size,
        "protect_system": settings.protect_system,
        "coordinator_writable": settings.coordinator_writable,
        "limits": limits_to_obj(&settings.limits),
    })
}

fn py_json_string(text: &str) -> String {
    // Python `json.dumps` default escaping (ensure_ascii=True).
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch if (ch as u32) < 0x7f => out.push(ch),
            ch => {
                let mut buffer = [0_u16; 2];
                for unit in ch.encode_utf16(&mut buffer) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

fn render_value(value: &Value, indent: usize) -> String {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let pad = "  ".repeat(indent + 1);
            let close = "  ".repeat(indent);
            // Keys arrive in the caller's insertion order for the
            // objects built here (serde_json preserves it in this
            // crate's maps only when constructed in order; the
            // settings object below is rendered by explicit order,
            // so this branch serves nested env/limits maps whose
            // Python order is key-sorted by construction).
            let parts: Vec<String> = map
                .iter()
                .map(|(key, item)| {
                    format!(
                        "{pad}{}: {}",
                        py_json_string(key),
                        render_value(item, indent + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{close}}}", parts.join(",\n"))
        }
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let rendered: Vec<String> = items
                .iter()
                .map(|item| render_value(item, indent + 1))
                .collect();
            // Python renders short scalar lists multi-line too under
            // indent=2; match its one-item-per-line form.
            let pad = "  ".repeat(indent + 1);
            let close = "  ".repeat(indent);
            let parts: Vec<String> = rendered.iter().map(|item| format!("{pad}{item}")).collect();
            format!("[\n{}\n{close}]", parts.join(",\n"))
        }
        Value::String(text) => py_json_string(text),
        other => other.to_string(),
    }
}

fn opt_string(value: &Option<String>) -> Value {
    value.clone().map_or(Value::Null, Value::String)
}

fn render_limits(limits: &SandboxLimits) -> String {
    // Python emits limits keys in dataclass field order, and each
    // quota object as {slot_prefix, cpu_quota} in that order.
    let quotas = if limits.slot_cpu_quotas.is_empty() {
        "[]".to_string()
    } else {
        let parts: Vec<String> = limits
            .slot_cpu_quotas
            .iter()
            .map(|(prefix, quota)| {
                format!(
                    "      {{\n        \"slot_prefix\": {},\n        \"cpu_quota\": {}\n      }}",
                    py_json_string(prefix),
                    py_json_string(quota)
                )
            })
            .collect();
        format!("[\n{}\n    ]", parts.join(",\n"))
    };
    let fields: Vec<(&str, String)> = vec![
        (
            "memory_max",
            render_value(&opt_string(&limits.memory_max), 2),
        ),
        (
            "memory_high",
            render_value(&opt_string(&limits.memory_high), 2),
        ),
        ("cpu_quota", render_value(&opt_string(&limits.cpu_quota), 2)),
        (
            "tasks_max",
            render_value(&limits.tasks_max.map_or(Value::Null, Value::from), 2),
        ),
        (
            "io_weight",
            render_value(&limits.io_weight.map_or(Value::Null, Value::from), 2),
        ),
        (
            "all_slots_memory_max",
            render_value(&opt_string(&limits.all_slots_memory_max), 2),
        ),
        (
            "all_slots_cpu_quota",
            render_value(&opt_string(&limits.all_slots_cpu_quota), 2),
        ),
        ("slot_cpu_quotas", quotas),
    ];
    let parts: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("    {}: {}", py_json_string(key), value))
        .collect();
    format!("{{\n{}\n  }}", parts.join(",\n"))
}

/// Render the settings in the exact key order Python's
/// `settings_to_obj` dict uses (a sorted map would not match).
pub(crate) fn render_settings_json(settings: &SandboxSettings) -> String {
    let strings = |items: &[String]| -> Value {
        Value::Array(
            items
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        )
    };
    // env appears in the settings' own pair order, as Python's
    // dict(env) does; a map would sort the names.
    let env = if settings.env.is_empty() {
        "{}".to_string()
    } else {
        let parts: Vec<String> = settings
            .env
            .iter()
            .map(|(key, value)| format!("    {}: {}", py_json_string(key), py_json_string(value)))
            .collect();
        format!("{{\n{}\n  }}", parts.join(",\n"))
    };
    let fields: Vec<(&str, String)> = vec![
        (
            "isolation",
            render_value(&Value::String(settings.isolation.clone()), 1),
        ),
        (
            "home",
            render_value(&Value::String(settings.home.clone()), 1),
        ),
        (
            "home_shared",
            render_value(&strings(&settings.home_shared), 1),
        ),
        (
            "home_private",
            render_value(&strings(&settings.home_private), 1),
        ),
        (
            "home_private_files",
            render_value(&strings(&settings.home_private_files), 1),
        ),
        (
            "home_hidden",
            render_value(&strings(&settings.home_hidden), 1),
        ),
        (
            "home_expose",
            render_value(&strings(&settings.home_expose), 1),
        ),
        ("outputs", render_value(&strings(&settings.outputs), 1)),
        (
            "read_write",
            render_value(&strings(&settings.read_write), 1),
        ),
        ("env", env),
        (
            "tmp_size",
            render_value(&Value::String(settings.tmp_size.clone()), 1),
        ),
        (
            "protect_system",
            render_value(&Value::Bool(settings.protect_system), 1),
        ),
        (
            "coordinator_writable",
            render_value(&Value::String(settings.coordinator_writable.clone()), 1),
        ),
        ("limits", render_limits(&settings.limits)),
    ];
    let parts: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("  {}: {}", py_json_string(key), value))
        .collect();
    format!("{{\n{}\n}}\n", parts.join(",\n"))
}

/// `wrkslots sandbox show-config` (#1101): the dispatch-shaped
/// handler. Input is the raw `sandbox` section (absent = defaults,
/// mirroring `_parse_sandbox_settings`); output is
/// `(exit, stdout, stderr)` byte-identical to the Python edition,
/// whose settings validation failure is a load refusal (exit 3).
pub(crate) fn sandbox_show_config(raw_section: Option<&Value>) -> (i32, String, String) {
    let settings = match raw_section {
        None => SandboxSettings::default(),
        Some(raw) => match settings_from_obj(raw) {
            Ok(settings) => settings,
            Err(message) => {
                return (
                    3,
                    String::new(),
                    format!(
                        "REFUSED: {message}\nREMEDY: run 'wrkslots sandbox --help', correct the named condition, then rerun 'wrkslots sandbox' with the required arguments\n"
                    ),
                );
            }
        },
    };
    (0, render_settings_json(&settings), String::new())
}

/// The `sandbox` section for a new project, mirroring Python
/// `default_config_obj`.
pub(crate) fn default_config_obj() -> Value {
    settings_to_obj(&SandboxSettings::default())
}

/// Add every missing default key, mirroring Python `merge_defaults`:
/// present keys are never changed, a present `limits` object gets
/// only its missing limit keys. Returns the merged section and the
/// dotted names added.
pub(crate) fn merge_defaults(existing: &Value) -> Result<(Value, Vec<String>), String> {
    let Some(map) = existing.as_object() else {
        return Err("configuration.sandbox must be an object".to_string());
    };
    let mut merged = map.clone();
    let mut added = Vec::new();
    let defaults = default_config_obj();
    let default_map = defaults.as_object().expect("default object");
    for (key, value) in default_map {
        match merged.get(key) {
            None => {
                merged.insert(key.clone(), value.clone());
                added.push(key.clone());
            }
            Some(current) if key == "limits" => {
                if let (Some(current_map), Some(default_limits)) =
                    (current.as_object(), value.as_object())
                {
                    let mut limits = current_map.clone();
                    for (limit_key, limit_value) in default_limits {
                        if !limits.contains_key(limit_key) {
                            limits.insert(limit_key.clone(), limit_value.clone());
                            added.push(format!("limits.{limit_key}"));
                        }
                    }
                    merged.insert(key.clone(), Value::Object(limits));
                }
            }
            _ => {}
        }
    }
    // The oracle validates the merged section before returning it.
    settings_from_obj(&Value::Object(merged.clone()))?;
    Ok((Value::Object(merged), added))
}

#[cfg(test)]
mod tests {
    const DRIVER: &str = r####"import argparse, io, json, os, sys
from contextlib import redirect_stderr, redirect_stdout
sys.path.insert(0, os.environ["WRKSLOTS_PY_ROOT"])
from wrkslots import cli, imagecmd, sandbox

case = sys.argv[1]
raw_cases = {
    "default": None,
    "custom": {
        "isolation": "root",
        "env": {"ALPHA": "~/x", "ZED": "1"},
        "limits": {"memory_max": "32G", "cpu_quota": "800%",
                   "slot_cpu_quotas": [{"slot_prefix": "wide", "cpu_quota": "3200%"}]},
        "home_expose": ["bin"],
    },
    "invalid": {"isolation": "namespace"},
}
raw = raw_cases[case]
full = {} if raw is None else {"sandbox": raw}
try:
    settings = cli._parse_sandbox_settings(full)
except cli.Refusal as exc:
    remedy = cli._refusal_remedy(argparse.Namespace(command="sandbox"), exc)
    sys.stdout.write(f"3\n---STDERR---\nREFUSED: {exc}\nREMEDY: {remedy}\n")
    sys.exit(0)
print = lambda *a, **k: sys.stdout.write(" ".join(str(x) for x in a) + "\n")
sys.stdout.write("0\n" + json.dumps(sandbox.settings_to_obj(settings), indent=2) + "\n---STDERR---\n")
"####;
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

    #[test]
    fn settings_round_trip_and_merge() {
        let settings = settings_from_obj(&json!({})).expect("defaults");
        assert_eq!(settings.isolation, "userns");
        assert_eq!(settings.home, "ro");
        assert_eq!(settings.tmp_size, "16G");
        let obj = settings_to_obj(&settings);
        assert_eq!(obj["limits"]["tasks_max"], 8192);
        assert_eq!(obj["coordinator_writable"], "worktrees");
        let parsed = settings_from_obj(&obj).expect("round trip");
        assert_eq!(parsed, settings);

        let (merged, added) = merge_defaults(&json!({"isolation": "root"})).expect("merge");
        assert_eq!(merged["isolation"], "root");
        assert!(added.contains(&"limits".to_string()), "{added:?}");
        assert!(settings_from_obj(&json!({"isolation": "namespace"})).is_err());
        let renamed = settings_from_obj(&json!({"memory_max": "32G"})).expect_err("renamed");
        assert!(renamed.contains("limits.memory_max"), "{renamed}");
    }

    /// Oracle differential (#1101): Python `_parse_sandbox_settings`
    /// plus `settings_to_obj` as a subprocess vs `sandbox_show_config`
    /// on the same raw section, byte-for-byte including the refusal.
    #[test]
    fn show_config_matches_python_oracle() {
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf();
        let dir =
            std::env::temp_dir().join(format!("wrkslotsd-show-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let driver_path = dir.join("driver.py");
        std::fs::write(&driver_path, DRIVER).expect("driver");
        let cases: Vec<(&str, Option<Value>)> = vec![
            ("default", None),
            (
                "custom",
                Some(json!({
                    "isolation": "root",
                    "env": {"ALPHA": "~/x", "ZED": "1"},
                    "limits": {"memory_max": "32G", "cpu_quota": "800%",
                               "slot_cpu_quotas": [{"slot_prefix": "wide", "cpu_quota": "3200%"}]},
                    "home_expose": ["bin"]
                })),
            ),
            ("invalid", Some(json!({"isolation": "namespace"}))),
        ];
        for (case, raw) in &cases {
            let output = std::process::Command::new("python3")
                .arg(&driver_path)
                .arg(case)
                .env("WRKSLOTS_PY_ROOT", repo.join("py"))
                .output()
                .expect("python oracle runs");
            let text = String::from_utf8(output.stdout).expect("utf8");
            let (head, rest) = text.split_once('\n').expect("exit line");
            let (stdout, stderr) = rest.split_once("---STDERR---\n").expect("marker");
            let expected = (
                head.trim().parse::<i32>().expect("exit"),
                stdout.to_string(),
                stderr.to_string(),
            );
            assert_eq!(sandbox_show_config(raw.as_ref()), expected, "case {case}");
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
