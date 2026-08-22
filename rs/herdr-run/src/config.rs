//! Strict per-project configuration discovery, parsing, and defaults.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::error::{HerdrRunError, Result};

/// Accepted configuration basenames, in preference order at each directory.
pub const CONFIG_FILENAMES: [&str; 2] = [".herdr-run.yaml", ".herdr-run.yml"];

/// The single `allow` entry that turns the allowlist off: any bare program name is admitted.
///
/// This is a named mode rather than something to be worked out, because the alternative — writing
/// out every program a project might ever want — is the kind of list people abandon halfway and
/// then widen by accident. It is deliberately the only entry permitted when it is present: an
/// `allow` reading `["*", "git"]` looks narrower than it is.
///
/// Everything except the program-name check still applies in this mode: terminal control
/// characters are still refused, the program must still be a bare name resolved from the pane's
/// `PATH`, wrapper prefixes must still be declared, and every `deny_*` rule still bites.
pub const ALLOW_ANY_PROGRAM: &str = "*";

/// Default ceiling on panes in the command workspace before a NEW tab is refused.
///
/// Every agent that ever runs a command leaves a tab behind and nothing closes it, so without a
/// ceiling the workspace grows for as long as agents are coined. The number is not arbitrary:
/// measured on devbig014 2026-08-10, a session with 260 panes drove the Herdr server to >1000% CPU
/// with every control call timing out. 32 keeps an eightfold margin below that while being far
/// more tabs than one project's agents legitimately need, and the cap is per workspace, so several
/// projects on one server each want headroom of their own.
pub const DEFAULT_MAX_PANES: u64 = 32;

/// Largest accepted `max_panes`. A shared finite bound keeps the two implementations identical.
pub const MAX_PANE_CAP: u64 = 1_000_000;

/// Largest command/readiness timeout accepted from configuration or the CLI.
///
/// This deliberately stays far below platform monotonic-clock limits so a finite timeout cannot
/// overflow into an accidental infinite wait.
pub const MAX_TIMEOUT_SECONDS: f64 = 31_536_000.0;

/// Fully resolved `herdr-run` configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Herdr workspace label holding this project's command tabs.
    pub workspace: String,
    /// Tab-label format supporting `{agent}` and `{project}`.
    pub tab_name: String,
    /// Optional command working directory; `None` leaves execution at the caller's directory.
    pub cwd: Option<String>,
    /// Bare program names admitted by policy.
    pub allow: Vec<String>,
    /// Wrapper programs that may precede an allowlisted program.
    pub prefixes: Vec<String>,
    /// Program-specific global options; Cargo entries are denied in every argument position.
    pub deny_global: BTreeMap<String, Vec<String>>,
    /// Program-specific subcommands denied by policy.
    pub deny_subcommand: BTreeMap<String, Vec<String>>,
    /// Options denied wherever they occur after the program.
    pub deny_anywhere: Vec<String>,
    /// Programs whose subcommand must appear in the corresponding positive list.
    pub allow_subcommand: BTreeMap<String, Vec<String>>,
    /// Global options whose value occupies the following token.
    pub value_options: BTreeMap<String, Vec<String>>,
    /// Directory holding run spools, audit log, and session cache.
    pub spool_dir: String,
    /// Maximum seconds to wait for a launched command to complete.
    pub timeout_seconds: f64,
    /// Days of per-run spool output to retain, pruned when a new run is written.
    pub retention_days: u64,
    /// Ceiling on panes in `workspace` before a NEW tab is refused; `0` disables the cap.
    ///
    /// Checked only when a tab has to be created, so an agent that already has its tab is never
    /// locked out of it. A cap that can break work in progress is a cap that gets switched off.
    pub max_panes: u64,
    /// Maximum seconds to wait for a pane to become ready.
    pub ready_timeout_seconds: f64,
    /// Readiness policy: `both` or `process`.
    pub readiness: String,
    /// Explicit rendered prompt suffix, or `None` to infer it.
    pub prompt_tail: Option<String>,
    /// Process names accepted as interactive shells.
    pub shells: Vec<String>,
    /// Remote used by the two-direction `doctor` probe.
    pub probe_remote: String,
    /// Herdr control-call broker: `direct` or `systemd-run`.
    pub broker: String,
    /// Absolute source configuration path, if a file supplied this configuration.
    pub source_path: Option<String>,
    /// Absolute project root used to resolve relative paths.
    pub project_root: String,
}

impl Config {
    /// Report whether `allow` has been set to the [`ALLOW_ANY_PROGRAM`] wildcard.
    #[must_use]
    pub fn allows_any_program(&self) -> bool {
        self.allow.iter().any(|entry| entry == ALLOW_ANY_PROGRAM)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace: "agent-cmds".to_owned(),
            tab_name: "{agent}".to_owned(),
            cwd: None,
            // Cargo is deliberately not a default: even dependency-oriented subcommands can
            // execute configured rustc wrappers, credential providers, or fetch helpers. The
            // cargo-specific policy below limits projects that explicitly accept that widening.
            allow: strings(&["git", "gh"]),
            prefixes: strings(&["with-proxy"]),
            deny_global: string_map(&[
                ("git", &["-c", "--config-env", "--exec-path", "--namespace"]),
                ("gh", &[]),
                ("cargo", &["--config", "-Z"]),
            ]),
            deny_subcommand: string_map(&[
                ("git", &["filter-branch", "daemon", "instaweb"]),
                ("gh", &["alias", "extension", "ext", "codespace", "cs"]),
            ]),
            deny_anywhere: strings(&["--upload-pack", "--receive-pack"]),
            allow_subcommand: string_map(&[(
                "cargo",
                &[
                    "fetch",
                    "update",
                    "generate-lockfile",
                    "vendor",
                    "metadata",
                    "tree",
                    "search",
                ],
            )]),
            value_options: string_map(&[
                (
                    "git",
                    &[
                        "-C",
                        "-c",
                        "--git-dir",
                        "--work-tree",
                        "--namespace",
                        "--exec-path",
                        "--config-env",
                    ],
                ),
                ("gh", &["-R", "--repo"]),
                (
                    "cargo",
                    &[
                        "--manifest-path",
                        "--config",
                        "-Z",
                        "-p",
                        "--package",
                        "--target-dir",
                    ],
                ),
            ]),
            spool_dir: ".herdr-run".to_owned(),
            timeout_seconds: 900.0,
            retention_days: crate::retention::RETENTION_DAYS,
            max_panes: DEFAULT_MAX_PANES,
            ready_timeout_seconds: 0.0,
            readiness: "both".to_owned(),
            prompt_tail: None,
            shells: strings(&["bash", "zsh", "sh", "dash", "fish", "ksh"]),
            probe_remote: "https://github.com/git/git".to_owned(),
            broker: "direct".to_owned(),
            source_path: None,
            project_root: ".".to_owned(),
        }
    }
}

/// YAML value decoder that rejects duplicate keys, merge keys, and custom tags before narrowing.
struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a YAML 1.2 scalar, sequence, or string-keyed mapping")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_i128<E>(self, value: i128) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        if let Ok(narrow) = i64::try_from(value) {
            return self.visit_i64(narrow);
        }
        self.visit_f64(value as f64)
    }

    fn visit_u128<E>(self, value: u128) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        if let Ok(narrow) = u64::try_from(value) {
            return self.visit_u64(narrow);
        }
        self.visit_f64(value as f64)
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = serde_json::Number::from_f64(value)
            .ok_or_else(|| E::custom("non-finite YAML numbers are not allowed"))?;
        Ok(StrictValue(Value::Number(number)))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut output = Vec::new();
        while let Some(item) = sequence.next_element::<StrictValue>()? {
            output.push(item.0);
        }
        Ok(StrictValue(Value::Array(output)))
    }

    fn visit_map<A>(self, mut mapping: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut output = Map::new();
        while let Some(key) = mapping.next_key::<String>()? {
            if key == "<<" {
                return Err(de::Error::custom(
                    "YAML merge keys ('<<') are not allowed in configuration",
                ));
            }
            if output.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate YAML mapping key {key:?}"
                )));
            }
            let value = mapping.next_value::<StrictValue>()?;
            output.insert(key, value.0);
        }
        Ok(StrictValue(Value::Object(output)))
    }

    fn visit_enum<A>(self, _data: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: de::EnumAccess<'de>,
    {
        Err(de::Error::custom(
            "custom YAML tags are not allowed in configuration",
        ))
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn string_map(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(key, values)| ((*key).to_owned(), strings(values)))
        .collect()
}

fn absolute_lexical(path: &Path) -> Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                HerdrRunError::config(format!("cannot determine current directory: {error}"))
            })?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.parent().is_some() {
                    normalized.pop();
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

/// Search `start` and its ancestors for the nearest accepted config filename.
#[must_use]
pub fn find_config_file(start: &Path) -> Option<PathBuf> {
    let mut current = absolute_lexical(start).ok()?;
    loop {
        for filename in CONFIG_FILENAMES {
            let candidate = current.join(filename);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Load an explicit configuration file, the nearest discovered file, or built-in defaults.
pub fn load_config(explicit_path: Option<&Path>, start_dir: &Path) -> Result<Config> {
    let start = absolute_lexical(start_dir)?;
    let path = if let Some(explicit) = explicit_path {
        if !explicit.is_file() {
            return Err(HerdrRunError::config(format!(
                "config file not found: {}",
                explicit.display()
            )));
        }
        Some(absolute_lexical(explicit)?)
    } else {
        find_config_file(&start)
    };

    let Some(path) = path else {
        return Ok(Config {
            project_root: start.to_string_lossy().into_owned(),
            ..Config::default()
        });
    };
    let text = fs::read_to_string(&path).map_err(|error| {
        HerdrRunError::config(format!(
            "cannot read config file {}: {error}",
            path.display()
        ))
    })?;
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        StrictValue::deserialize(serde_norway::Deserializer::from_str(&text))
    }));
    let document = match decoded {
        Ok(Ok(strict)) => strict.0,
        Ok(Err(error)) => {
            return Err(HerdrRunError::config(format!(
                "{}: invalid YAML: {error}",
                path.display()
            )));
        }
        Err(_) => {
            return Err(HerdrRunError::config(format!(
                "{}: invalid YAML: parser failed internally",
                path.display()
            )));
        }
    };
    let root = path.parent().unwrap_or_else(|| Path::new("/"));
    parse_config(
        &document,
        Some(path.to_string_lossy().into_owned()),
        root.to_string_lossy().into_owned(),
    )
}

/// Validate an already decoded YAML/JSON value and construct a resolved configuration.
pub fn parse_config(
    document: &Value,
    source_path: Option<String>,
    project_root: String,
) -> Result<Config> {
    let mut config = Config {
        source_path: source_path.clone(),
        project_root,
        ..Config::default()
    };
    if document.is_null() {
        return Ok(config);
    }
    let what = source_path.as_deref().unwrap_or("<config>");
    let mapping = require_object(document, what)?;
    reject_unknown_keys(mapping, what)?;

    if let Some(value) = mapping.get("workspace") {
        config.workspace = require_text(value, &format!("{what}.workspace"))?;
        require_nonempty(&config.workspace, &format!("{what}.workspace"))?;
    }
    if let Some(value) = mapping.get("tab_name") {
        config.tab_name = require_text(value, &format!("{what}.tab_name"))?;
        render_tab_name(&config.tab_name, "agent", "project")?;
    }
    if let Some(value) = mapping.get("cwd") {
        config.cwd = require_optional_text(value, &format!("{what}.cwd"))?;
    }
    if let Some(value) = mapping.get("allow") {
        config.allow = require_policy_list(value, &format!("{what}.allow"))?;
    }
    if let Some(value) = mapping.get("prefixes") {
        config.prefixes = require_policy_list(value, &format!("{what}.prefixes"))?;
    }
    if let Some(value) = mapping.get("deny_global") {
        config.deny_global = require_string_map(value, &format!("{what}.deny_global"))?;
    }
    if let Some(value) = mapping.get("deny_subcommand") {
        config.deny_subcommand = require_string_map(value, &format!("{what}.deny_subcommand"))?;
    }
    if let Some(value) = mapping.get("deny_anywhere") {
        config.deny_anywhere = require_policy_list(value, &format!("{what}.deny_anywhere"))?;
    }
    if let Some(value) = mapping.get("allow_subcommand") {
        config.allow_subcommand = require_string_map(value, &format!("{what}.allow_subcommand"))?;
    }
    if let Some(value) = mapping.get("value_options") {
        config.value_options = require_string_map(value, &format!("{what}.value_options"))?;
    }
    if let Some(value) = mapping.get("spool_dir") {
        config.spool_dir = require_text(value, &format!("{what}.spool_dir"))?;
    }
    if let Some(value) = mapping.get("timeout_seconds") {
        config.timeout_seconds = require_number(value, &format!("{what}.timeout_seconds"))?;
    }
    if let Some(value) = mapping.get("retention_days") {
        config.retention_days =
            require_nonnegative_integer(value, &format!("{what}.retention_days"))?;
    }
    if let Some(value) = mapping.get("max_panes") {
        config.max_panes =
            require_bounded_count(value, &format!("{what}.max_panes"), MAX_PANE_CAP)?;
    }
    if let Some(value) = mapping.get("ready_timeout_seconds") {
        config.ready_timeout_seconds =
            require_number(value, &format!("{what}.ready_timeout_seconds"))?;
    }
    if let Some(value) = mapping.get("readiness") {
        config.readiness =
            require_choice(value, &format!("{what}.readiness"), &["both", "process"])?;
    }
    if let Some(value) = mapping.get("prompt_tail") {
        config.prompt_tail = require_optional_text(value, &format!("{what}.prompt_tail"))?;
    }
    if let Some(value) = mapping.get("shells") {
        config.shells = require_policy_list(value, &format!("{what}.shells"))?;
    }
    if let Some(value) = mapping.get("probe_remote") {
        config.probe_remote = require_text(value, &format!("{what}.probe_remote"))?;
    }
    if let Some(value) = mapping.get("broker") {
        config.broker =
            require_choice(value, &format!("{what}.broker"), &["direct", "systemd-run"])?;
    }
    if config.allow.is_empty() {
        return Err(HerdrRunError::config(format!(
            "{what}.allow: refusing an EMPTY allowlist — no command could ever run"
        )));
    }
    if config.allows_any_program() && config.allow.len() > 1 {
        return Err(HerdrRunError::config(format!(
            "{what}.allow: {ALLOW_ANY_PROGRAM:?} already admits every program, so it must be the only entry; listing programs beside it makes the policy look narrower than it is"
        )));
    }
    if config.allow.iter().any(|program| program == "cargo")
        && !config.allow_subcommand.contains_key("cargo")
    {
        return Err(HerdrRunError::config(format!(
            "{what}.allow_subcommand: cargo is allowed but has no positive subcommand list"
        )));
    }
    Ok(config)
}

fn reject_unknown_keys(mapping: &Map<String, Value>, what: &str) -> Result<()> {
    const KNOWN: [&str; 20] = [
        "workspace",
        "tab_name",
        "cwd",
        "allow",
        "prefixes",
        "deny_global",
        "deny_subcommand",
        "deny_anywhere",
        "allow_subcommand",
        "value_options",
        "spool_dir",
        "timeout_seconds",
        "retention_days",
        "max_panes",
        "ready_timeout_seconds",
        "readiness",
        "prompt_tail",
        "shells",
        "broker",
        "probe_remote",
    ];
    if mapping.keys().any(|key| contains_terminal_control(key)) {
        return Err(HerdrRunError::config(format!(
            "{what}: control characters are not allowed in keys"
        )));
    }
    let known: BTreeSet<&str> = KNOWN.iter().copied().collect();
    let mut unknown: Vec<&str> = mapping
        .keys()
        .map(String::as_str)
        .filter(|key| !known.contains(key))
        .collect();
    unknown.sort_unstable();
    if unknown.is_empty() {
        return Ok(());
    }
    let known_names = known.into_iter().collect::<Vec<_>>().join(", ");
    Err(HerdrRunError::config(format!(
        "{what}: unknown key(s): {}. Known keys: {known_names}",
        unknown.join(", ")
    )))
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn contains_terminal_control(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(u32::from(character), 0x00..=0x1f | 0x7f..=0x9f))
}

fn require_object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| {
        HerdrRunError::config(format!(
            "{what}: expected an object, got {}",
            value_type(value)
        ))
    })
}

fn require_text(value: &Value, what: &str) -> Result<String> {
    let text = value.as_str().ok_or_else(|| {
        HerdrRunError::config(format!(
            "{what}: must be a string, got {}",
            value_type(value)
        ))
    })?;
    if contains_terminal_control(text) {
        return Err(HerdrRunError::config(format!(
            "{what}: control characters are not allowed"
        )));
    }
    Ok(text.to_owned())
}

fn require_optional_text(value: &Value, what: &str) -> Result<Option<String>> {
    if value.is_null() {
        Ok(None)
    } else {
        require_text(value, what).map(Some)
    }
}

fn require_string_list(value: &Value, what: &str) -> Result<Vec<String>> {
    let values = value.as_array().ok_or_else(|| {
        HerdrRunError::config(format!(
            "{what}: expected an array, got {}",
            value_type(value)
        ))
    })?;
    values
        .iter()
        .map(|item| match item.as_str() {
            Some(text) if contains_terminal_control(text) => Err(HerdrRunError::config(format!(
                "{what}: control characters are not allowed"
            ))),
            Some(text) => Ok(text.to_owned()),
            None => Err(HerdrRunError::config(format!(
                "{what}: every entry must be a string, got {}",
                value_type(item)
            ))),
        })
        .collect()
}

fn require_policy_list(value: &Value, what: &str) -> Result<Vec<String>> {
    let values = require_string_list(value, what)?;
    for item in &values {
        require_nonempty(item, what)?;
    }
    Ok(values)
}

fn require_string_map(value: &Value, what: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let mapping = require_object(value, what)?;
    mapping
        .iter()
        .map(|(key, item)| {
            require_nonempty(key, what)?;
            if contains_terminal_control(key) {
                return Err(HerdrRunError::config(format!(
                    "{what}: control characters are not allowed"
                )));
            }
            require_policy_list(item, &format!("{what}.{key}")).map(|items| (key.clone(), items))
        })
        .collect()
}

fn require_number(value: &Value, what: &str) -> Result<f64> {
    let number = value.as_f64().ok_or_else(|| {
        HerdrRunError::config(format!(
            "{what}: must be a number, got {}",
            value_type(value)
        ))
    })?;
    if !number.is_finite() {
        return Err(HerdrRunError::config(format!("{what}: must be finite")));
    }
    if number < 0.0 {
        return Err(HerdrRunError::config(format!(
            "{what}: must not be negative"
        )));
    }
    if number > MAX_TIMEOUT_SECONDS {
        return Err(HerdrRunError::config(format!(
            "{what}: must not exceed {MAX_TIMEOUT_SECONDS:.0} seconds"
        )));
    }
    Ok(number)
}

fn require_nonnegative_integer(value: &Value, what: &str) -> Result<u64> {
    let number = value.as_u64().ok_or_else(|| {
        HerdrRunError::config(format!(
            "{what}: must be a non-negative integer, got {}",
            value_type(value)
        ))
    })?;
    if number > crate::retention::MAX_RETENTION_DAYS {
        return Err(HerdrRunError::config(format!(
            "{what}: must not exceed {} days",
            crate::retention::MAX_RETENTION_DAYS
        )));
    }
    Ok(number)
}

fn require_bounded_count(value: &Value, what: &str, limit: u64) -> Result<u64> {
    let number = value.as_u64().ok_or_else(|| {
        HerdrRunError::config(format!(
            "{what}: must be a non-negative integer, got {}",
            value_type(value)
        ))
    })?;
    if number > limit {
        return Err(HerdrRunError::config(format!(
            "{what}: must not exceed {limit}"
        )));
    }
    Ok(number)
}

fn require_nonempty(value: &str, what: &str) -> Result<()> {
    if value.is_empty() {
        Err(HerdrRunError::config(format!(
            "{what}: entries must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn validate_tab_template(template: &str, what: &str) -> Result<()> {
    require_nonempty(template, what)?;
    let mut remainder = template;
    while let Some(index) = remainder.find(['{', '}']) {
        let candidate = &remainder[index..];
        if let Some(rest) = candidate.strip_prefix("{{") {
            remainder = rest;
        } else if let Some(rest) = candidate.strip_prefix("}}") {
            remainder = rest;
        } else if let Some(rest) = candidate.strip_prefix("{agent}") {
            remainder = rest;
        } else if let Some(rest) = candidate.strip_prefix("{project}") {
            remainder = rest;
        } else {
            return Err(HerdrRunError::config(format!(
                "{what}: only exact {{agent}} and {{project}} placeholders are allowed"
            )));
        }
    }
    Ok(())
}

/// Render a tab-label schema restricted to literal text and plain `{agent}`/`{project}` fields.
pub fn render_tab_name(schema: &str, agent: &str, project: &str) -> Result<String> {
    validate_tab_template(schema, "tab_name schema")?;
    let mut output = String::new();
    let mut remainder = schema;
    while let Some(index) = remainder.find(['{', '}']) {
        output.push_str(&remainder[..index]);
        let candidate = &remainder[index..];
        if let Some(rest) = candidate.strip_prefix("{{") {
            output.push('{');
            remainder = rest;
        } else if let Some(rest) = candidate.strip_prefix("}}") {
            output.push('}');
            remainder = rest;
        } else if let Some(rest) = candidate.strip_prefix("{agent}") {
            output.push_str(agent);
            remainder = rest;
        } else if let Some(rest) = candidate.strip_prefix("{project}") {
            output.push_str(project);
            remainder = rest;
        } else {
            // Validation above makes this unreachable, but return a typed configuration error if
            // this parser and its validator ever drift rather than relying on an assertion.
            return Err(HerdrRunError::config(
                "tab_name schema may use only plain {agent} and {project} placeholders",
            ));
        }
    }
    output.push_str(remainder);
    if contains_terminal_control(&output) {
        return Err(HerdrRunError::config(
            "rendered tab_name: control characters are not allowed",
        ));
    }
    require_nonempty(&output, "rendered tab_name")?;
    Ok(output)
}

fn require_choice(value: &Value, what: &str, allowed: &[&str]) -> Result<String> {
    let choice = require_text(value, what)?;
    if allowed.contains(&choice.as_str()) {
        Ok(choice)
    } else {
        Err(HerdrRunError::config(format!(
            "{what}: must be one of {}; got {choice:?}",
            allowed.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "herdr-run-config-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        path
    }

    #[test]
    fn defaults_match_the_python_policy() {
        let config = Config::default();
        assert_eq!(config.workspace, "agent-cmds");
        assert_eq!(config.tab_name, "{agent}");
        assert_eq!(config.allow, strings(&["git", "gh"]));
        assert_eq!(
            config.allow_subcommand.get("cargo").unwrap(),
            &strings(&[
                "fetch",
                "update",
                "generate-lockfile",
                "vendor",
                "metadata",
                "tree",
                "search",
            ])
        );
        assert_eq!(config.prefixes, strings(&["with-proxy"]));
        assert_eq!(config.timeout_seconds, 900.0);
        assert_eq!(config.retention_days, 4);
        // Named literally rather than compared against DEFAULT_MAX_PANES: a test that reads the
        // production constant would agree with any value the constant ever takes, including a
        // typo, and so would pin nothing at all.
        assert_eq!(config.max_panes, 32);
        assert_eq!(config.readiness, "both");
        assert_eq!(config.broker, "direct");
    }

    #[test]
    fn parses_every_supported_key_and_replaces_maps_wholesale() {
        let document = serde_json::json!({
            "workspace": "cmds", "tab_name": "{project}-{agent}", "cwd": "sub",
            "allow": ["cargo"], "prefixes": [], "deny_global": {"cargo": ["-Z"]},
            "deny_subcommand": {"cargo": ["install"]}, "deny_anywhere": ["--evil"],
            "allow_subcommand": {"cargo": ["fetch", "build"]},
            "value_options": {"cargo": ["--manifest-path"]}, "spool_dir": "out",
            "timeout_seconds": 30, "retention_days": 7,
            "ready_timeout_seconds": 5, "readiness": "process",
            "prompt_tail": "% ", "shells": ["zsh"], "broker": "systemd-run",
            "probe_remote": "https://example.test/repo"
        });
        let config = parse_config(&document, Some("x.yaml".into()), "/tmp".into()).unwrap();
        assert_eq!(config.allow, strings(&["cargo"]));
        assert_eq!(config.deny_global.len(), 1);
        assert!(!config.deny_global.contains_key("git"));
        assert_eq!(
            config.allow_subcommand["cargo"],
            strings(&["fetch", "build"])
        );
        assert_eq!(config.timeout_seconds, 30.0);
        assert_eq!(config.retention_days, 7);
        assert_eq!(config.prompt_tail.as_deref(), Some("% "));
    }

    #[test]
    fn every_malformed_shape_is_a_typed_config_error() {
        let cases = [
            serde_json::json!({"allow": "git"}),
            serde_json::json!({"allow": [1]}),
            serde_json::json!({"workspace": 42}),
            serde_json::json!({"timeout_seconds": true}),
            serde_json::json!({"timeout_seconds": -1}),
            serde_json::json!({"timeout_seconds": MAX_TIMEOUT_SECONDS + 1.0}),
            serde_json::json!({"retention_days": -1}),
            serde_json::json!({"retention_days": 1.5}),
            serde_json::json!({"retention_days": "4"}),
            serde_json::json!({"retention_days": crate::retention::MAX_RETENTION_DAYS + 1}),
            serde_json::json!({"readiness": "maybe"}),
            serde_json::json!({"deny_global": ["git"]}),
            serde_json::json!({"allow": []}),
            serde_json::json!({"allowlist": ["git"]}),
        ];
        for document in cases {
            let error = parse_config(&document, Some("x.yaml".into()), "/tmp".into())
                .expect_err("malformed config must fail");
            assert_eq!(error.exit_code(), crate::error::EXIT_CONFIG);
        }
    }

    #[test]
    fn the_allow_wildcard_must_stand_alone() {
        let error = parse_config(
            &serde_json::json!({"allow": ["*", "git"]}),
            Some("x.yaml".into()),
            "/tmp".into(),
        )
        .expect_err("a wildcard mixed with named programs must fail");
        assert_eq!(error.exit_code(), crate::error::EXIT_CONFIG);
        assert!(
            error.message().contains("must be the only entry"),
            "{error}"
        );

        let wildcard = parse_config(
            &serde_json::json!({"allow": ["*"]}),
            Some("x.yaml".into()),
            "/tmp".into(),
        )
        .expect("a lone wildcard is a valid allowlist");
        assert!(wildcard.allows_any_program());
        assert!(!Config::default().allows_any_program());
    }

    #[test]
    fn cargo_allow_requires_its_positive_subcommand_map() {
        let document = serde_json::json!({
            "allow": ["git", "cargo"],
            "allow_subcommand": {"custom-tool": ["inspect"]}
        });
        let error = parse_config(&document, Some("x.yaml".into()), "/tmp".into()).unwrap_err();
        assert!(error
            .message()
            .contains("cargo is allowed but has no positive"));
    }

    #[test]
    fn nearest_config_wins_and_yaml_name_precedes_yml() {
        let root = temporary_directory("nearest");
        let nested = root.join("slot/deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join(".herdr-run.yaml"), "workspace: outer\n").unwrap();
        fs::write(root.join(".herdr-run.yml"), "workspace: wrong-name\n").unwrap();
        fs::write(root.join("slot/.herdr-run.yaml"), "workspace: inner\n").unwrap();
        let config = load_config(None, &nested).unwrap();
        assert_eq!(config.workspace, "inner");
        assert_eq!(config.project_root, root.join("slot").to_string_lossy());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_missing_and_invalid_yaml_are_code_78_errors() {
        let root = temporary_directory("invalid");
        let missing = root.join("missing.yaml");
        assert_eq!(
            load_config(Some(&missing), &root).unwrap_err().exit_code(),
            crate::error::EXIT_CONFIG
        );
        let invalid = root.join("invalid.yaml");
        fs::write(&invalid, "allow: [unterminated\n").unwrap();
        assert_eq!(
            load_config(Some(&invalid), &root).unwrap_err().exit_code(),
            crate::error::EXIT_CONFIG
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_merge_tag_and_nonfinite_yaml_are_rejected() {
        for (label, text) in [
            ("duplicate", "allow: [git]\nallow: [gh]\n"),
            (
                "nested-duplicate",
                "deny_global:\n  git: [-c]\n  git: [--exec-path]\n",
            ),
            (
                "merge",
                "base: &base {git: [-c]}\ndeny_global:\n  <<: *base\n",
            ),
            ("tag", "workspace: !custom value\n"),
            ("infinity", "timeout_seconds: .inf\n"),
            ("nan", "timeout_seconds: .nan\n"),
        ] {
            let root = temporary_directory(label);
            let path = root.join(".herdr-run.yaml");
            fs::write(&path, text).unwrap();
            let error = load_config(Some(&path), &root).expect_err(label);
            assert_eq!(
                error.exit_code(),
                crate::error::EXIT_CONFIG,
                "{label}: {error}"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn yaml_uses_core_12_scalar_resolution() {
        let root = temporary_directory("yaml-core");
        let path = root.join(".herdr-run.yaml");
        fs::write(
            &path,
            "workspace: yes\nallow: [git, on, off]\ntimeout_seconds: 0o10\n",
        )
        .unwrap();
        let config = load_config(Some(&path), &root).unwrap();
        assert_eq!(config.workspace, "yes");
        assert_eq!(config.allow, strings(&["git", "on", "off"]));
        assert_eq!(config.timeout_seconds, 8.0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn templates_empty_policy_names_and_controls_are_rejected() {
        for document in [
            serde_json::json!({"tab_name": "{agent.name}"}),
            serde_json::json!({"tab_name": "{agent!r}"}),
            serde_json::json!({"tab_name": "{agent:>10}"}),
            serde_json::json!({"tab_name": "{unknown}"}),
            serde_json::json!({"tab_name": "x}"}),
            serde_json::json!({"tab_name": "x{"}),
            serde_json::json!({"allow": [""]}),
            serde_json::json!({"prefixes": ["with\u{001b}proxy"]}),
            serde_json::json!({"deny_global": {"": ["-c"]}}),
            serde_json::json!({"shells": ["ba\u{0085}sh"]}),
        ] {
            let error = parse_config(&document, Some("x.yaml".into()), "/tmp".into())
                .expect_err("unsafe config must fail");
            assert_eq!(error.exit_code(), crate::error::EXIT_CONFIG);
        }
        for valid in [
            "literal",
            "{agent}",
            "{project}-{agent}",
            "x{agent}y{project}z",
            "{{literal}}-{agent}",
            "{{{agent}}}",
        ] {
            let config = parse_config(
                &serde_json::json!({"tab_name": valid}),
                Some("x.yaml".into()),
                "/tmp".into(),
            )
            .unwrap();
            assert_eq!(config.tab_name, valid);
        }
        assert_eq!(
            render_tab_name("{{{agent}}}-{project}", "release", "repo").unwrap(),
            "{release}-repo"
        );
        assert!(render_tab_name("{agent}", "bad\nagent", "repo").is_err());
    }

    #[test]
    fn empty_yaml_uses_defaults_but_records_its_source() {
        let root = temporary_directory("empty");
        let path = root.join(".herdr-run.yaml");
        fs::write(&path, "").unwrap();
        let config = load_config(None, &root).unwrap();
        assert_eq!(config.allow, strings(&["git", "gh"]));
        let expected_source = path.to_string_lossy().into_owned();
        assert_eq!(
            config.source_path.as_deref(),
            Some(expected_source.as_str())
        );
        fs::remove_dir_all(root).unwrap();
    }
}
