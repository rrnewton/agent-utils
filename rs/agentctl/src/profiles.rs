//! Strict project-local launch profiles.
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::agent::AgentError;

pub(crate) const SCHEMA: &str = "agentctl-profiles/v1";
const PROFILE_PATH: &str = ".agentctl/profiles.json";
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const SYSTEM_GIT: &str = "/usr/bin/git";

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

#[derive(Clone, Debug)]
pub(crate) struct LaunchProfile {
    pub(crate) name: String,
    pub(crate) harness: String,
    pub(crate) mode: String,
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) argv: Vec<String>,
    pub(crate) environment: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PublicProfile {
    name: String,
    harness: String,
    mode: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    argv_count: usize,
    environment: Vec<String>,
}

impl LaunchProfile {
    pub(crate) fn public(&self) -> PublicProfile {
        PublicProfile {
            name: self.name.clone(),
            harness: self.harness.clone(),
            mode: self.mode.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            argv_count: self.argv.len(),
            environment: self
                .environment
                .iter()
                .filter_map(|entry| entry.split_once('=').map(|(name, _)| name.to_owned()))
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: String,
    profiles: UniqueMap<RawProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    harness: String,
    mode: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    #[serde(default)]
    argv: Vec<String>,
    #[serde(default)]
    env: UniqueMap<String>,
}

#[derive(Debug, Default)]
struct UniqueMap<V>(BTreeMap<String, V>);

impl<'de, V: Deserialize<'de>> Deserialize<'de> for UniqueMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueMapVisitor<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for UniqueMapVisitor<V> {
            type Value = UniqueMap<V>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object with unique keys")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, V>()? {
                    if values.insert(key.clone(), value).is_some() {
                        return Err(A::Error::custom(format!("duplicate key {key:?}")));
                    }
                }
                Ok(UniqueMap(values))
            }
        }
        deserializer.deserialize_map(UniqueMapVisitor(std::marker::PhantomData))
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && !value.contains('\0')
}

fn valid_env_name(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn secret_name(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "API_KEY",
        "PRIVATE_KEY",
    ]
    .iter()
    .any(|needle| upper.contains(needle))
}

fn secret_option(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "--api-key",
        "--api_key",
        "--token",
        "--secret",
        "--password",
        "--passwd",
        "--credential",
    ]
    .iter()
    .any(|option| lower == *option || lower.starts_with(&format!("{option}=")))
}

fn value_option(value: &str, short: &str, long: &str) -> bool {
    value == short
        || value == long
        || value
            .strip_prefix(long)
            .is_some_and(|rest| rest.starts_with('='))
        || (!value.starts_with("--") && value.starts_with(short) && value.len() > short.len())
}

fn codex_config_key(argv: &[String], index: usize) -> Option<&str> {
    let item = argv.get(index)?.as_str();
    let assignment = if matches!(item, "-c" | "--config") {
        argv.get(index + 1)?.as_str()
    } else if let Some(value) = item.strip_prefix("--config=") {
        value
    } else if !item.starts_with("--") {
        let value = item.strip_prefix("-c")?;
        value.strip_prefix('=').unwrap_or(value)
    } else {
        return None;
    };
    assignment.split_once('=').map(|(key, _)| {
        let key = key.trim();
        key.strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                key.strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(key)
    })
}

pub(crate) fn validate_raw_harness_arguments(
    label: &str,
    harness: &str,
    argv: &[String],
    structured_model: bool,
    structured_effort: bool,
    structured_resume: bool,
) -> Result<(), AgentError> {
    validate_structured_harness_argument_conflicts(
        label,
        harness,
        argv,
        structured_model,
        structured_effort,
        structured_resume,
    )?;
    let option_keys = argv
        .iter()
        .filter(|value| value.starts_with('-'))
        .map(|value| value.split_once('=').map_or(value.as_str(), |(key, _)| key))
        .collect::<Vec<_>>();
    let config_keys = if harness == "codex" {
        (0..argv.len())
            .filter_map(|index| codex_config_key(argv, index))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let has_codex_config = harness == "codex"
        && argv
            .iter()
            .any(|value| value_option(value, "-c", "--config"));
    if argv
        .iter()
        .any(|value| value_option(value, "-m", "--model"))
        || config_keys.contains(&"model")
    {
        return Err(fail(format!("{label} must set model with the model field")));
    }
    if option_keys
        .iter()
        .any(|key| matches!(*key, "--reasoning-effort" | "--effort"))
        || argv
            .iter()
            .any(|value| value.starts_with("model_reasoning_effort="))
        || config_keys.contains(&"model_reasoning_effort")
    {
        return Err(fail(format!(
            "{label} must set reasoning effort with the reasoning_effort field"
        )));
    }
    if harness == "codex"
        && (structured_model || structured_effort)
        && (has_codex_config
            || argv
                .iter()
                .any(|value| value_option(value, "-p", "--profile")))
    {
        return Err(fail(format!(
            "{label} cannot combine structured model or reasoning effort with raw Codex config or profile arguments"
        )));
    }
    Ok(())
}

pub(crate) fn validate_structured_harness_argument_conflicts(
    label: &str,
    harness: &str,
    argv: &[String],
    structured_model: bool,
    structured_effort: bool,
    structured_resume: bool,
) -> Result<(), AgentError> {
    let option_keys = argv
        .iter()
        .filter(|value| value.starts_with('-'))
        .map(|value| value.split_once('=').map_or(value.as_str(), |(key, _)| key))
        .collect::<Vec<_>>();
    let config_keys = if harness == "codex" {
        (0..argv.len())
            .filter_map(|index| codex_config_key(argv, index))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if structured_model
        && (argv
            .iter()
            .any(|value| value_option(value, "-m", "--model"))
            || config_keys.contains(&"model"))
    {
        return Err(fail(format!("{label} must set model with the model field")));
    }
    if structured_effort
        && (option_keys
            .iter()
            .any(|key| matches!(*key, "--reasoning-effort" | "--effort"))
            || argv
                .iter()
                .any(|value| value.starts_with("model_reasoning_effort="))
            || config_keys.contains(&"model_reasoning_effort"))
    {
        return Err(fail(format!(
            "{label} must set reasoning effort with the reasoning_effort field"
        )));
    }
    if harness == "codex"
        && (structured_model || structured_effort)
        && (config_keys.contains(&"profile")
            || argv
                .iter()
                .any(|value| value_option(value, "-p", "--profile")))
    {
        return Err(fail(format!(
            "{label} cannot combine structured model or reasoning effort with a raw Codex profile selector"
        )));
    }
    let duplicate_resume = (harness == "codex" && argv.iter().any(|value| value == "resume"))
        || (harness == "claude"
            && argv.iter().any(|value| {
                value_option(value, "-r", "--resume")
                    || matches!(value.as_str(), "-c" | "--continue")
            }));
    if structured_resume && duplicate_resume {
        return Err(fail(format!(
            "{label} cannot repeat the structured resume selector in raw arguments"
        )));
    }
    Ok(())
}

fn require_ignored(cwd: &Path, path: &Path) -> Result<(), AgentError> {
    let relative = path
        .strip_prefix(cwd)
        .map_err(|_| fail("profile config must remain below the selected working directory"))?;
    let git = fs::canonicalize(SYSTEM_GIT).map_err(|error| {
        fail(format!(
            "cannot verify the system Git executable {SYSTEM_GIT}: {error}"
        ))
    })?;
    let metadata = fs::metadata(&git).map_err(|error| {
        fail(format!(
            "cannot inspect the system Git executable {}: {error}",
            git.display()
        ))
    })?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o022 != 0
    {
        return Err(fail(format!(
            "refusing unsafe Git executable: {}",
            git.display()
        )));
    }
    let mut child = Command::new(git)
        .args(["-C"])
        .arg(cwd)
        .args(["check-ignore", "--quiet", "--"])
        .arg(relative)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            fail(format!(
                "cannot verify that profile config is ignored: {error}"
            ))
        })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| fail(format!("cannot wait for git check-ignore: {error}")))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(fail("git check-ignore timed out after 5 seconds"));
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        return Err(fail(format!(
            "profile config is not ignored by Git: {}; add .agentctl/ to .gitignore",
            relative.display()
        )));
    }
    Ok(())
}

fn validate_muse_headless_arguments(name: &str, argv: &[String]) -> Result<(), AgentError> {
    let mut seen_singletons = std::collections::BTreeSet::new();
    let mut index = 0;
    while index < argv.len() {
        let item = &argv[index];
        let key = item.split_once('=').map_or(item.as_str(), |(key, _)| key);
        if matches!(
            key,
            "--" | "--api-key-stdin" | "--json" | "--session-id" | "--prompt-file"
        ) || matches!(item.as_str(), "exec" | "resume")
        {
            return Err(fail(format!(
                "profile {name:?} cannot override runner-owned option {key:?}"
            )));
        }
        if matches!(item.as_str(), "--model" | "--reasoning-effort" | "--effort") {
            if !seen_singletons.insert(key) {
                return Err(fail(format!(
                    "profile {name:?} repeats singleton option {key:?}; precedence must be explicit"
                )));
            }
            if argv
                .get(index + 1)
                .is_none_or(|value| value.starts_with('-'))
            {
                return Err(fail(format!(
                    "profile {name:?} option {key:?} requires one value"
                )));
            }
            index += 2;
            continue;
        }
        if !item.starts_with('-') {
            return Err(fail(format!(
                "profile {name:?} argv must use --option=value for options with values; positional arguments are reserved for the prompt"
            )));
        }
        if matches!(key, "--model" | "--reasoning-effort" | "--effort")
            && !seen_singletons.insert(key)
        {
            return Err(fail(format!(
                "profile {name:?} repeats singleton option {key:?}; precedence must be explicit"
            )));
        }
        index += 1;
    }
    Ok(())
}

fn validate(name: String, raw: RawProfile) -> Result<LaunchProfile, AgentError> {
    if !valid_name(&name) {
        return Err(fail(format!(
            "invalid profile name {name:?}; use lowercase letters, digits, and hyphens"
        )));
    }
    if !matches!(raw.harness.as_str(), "codex" | "claude" | "muse" | "agy") {
        return Err(fail(format!(
            "profile {name:?} has unsupported harness {:?}",
            raw.harness
        )));
    }
    if !matches!(raw.mode.as_str(), "interactive" | "headless") {
        return Err(fail(format!(
            "profile {name:?} has unsupported mode {:?}",
            raw.mode
        )));
    }
    let combination_supported = if raw.mode == "interactive" {
        matches!(raw.harness.as_str(), "codex" | "claude" | "muse")
    } else {
        matches!(raw.harness.as_str(), "codex" | "agy" | "muse")
    };
    if !combination_supported {
        return Err(fail(format!(
            "profile {name:?} has unsupported harness/mode combination {:?}/{:?}",
            raw.harness, raw.mode
        )));
    }
    if !valid_text(&raw.harness)
        || !valid_text(&raw.mode)
        || raw.model.as_deref().is_some_and(|value| !valid_text(value))
        || raw
            .reasoning_effort
            .as_deref()
            .is_some_and(|value| !valid_text(value))
        || raw.argv.iter().any(|value| !valid_text(value))
    {
        return Err(fail(format!(
            "profile {name:?} fields must be nonempty NUL-free strings"
        )));
    }
    if raw.reasoning_effort.as_deref().is_some_and(|value| {
        !matches!(
            value,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
    }) {
        return Err(fail(format!(
            "profile {name:?} has unsupported reasoning effort {:?}",
            raw.reasoning_effort
        )));
    }
    if raw.mode == "headless" && raw.harness != "muse" && !raw.argv.is_empty() {
        return Err(fail(format!(
            "profile {name:?} headless {} does not support raw argv",
            raw.harness
        )));
    }
    if raw.mode == "headless" && raw.harness != "muse" && raw.reasoning_effort.is_some() {
        return Err(fail(format!(
            "profile {name:?} headless {} does not support reasoning_effort",
            raw.harness
        )));
    }
    if raw.mode == "headless" && !raw.env.0.is_empty() {
        return Err(fail(format!(
            "profile {name:?} headless mode does not support environment entries"
        )));
    }
    if raw.argv.iter().enumerate().any(|(index, value)| {
        secret_option(value)
            || index
                .checked_sub(1)
                .is_some_and(|prior| secret_option(&raw.argv[prior]))
    }) {
        return Err(fail(format!(
            "profile {name:?} argv appears to contain a secret; use the harness credential store"
        )));
    }
    validate_raw_harness_arguments(
        &format!("profile {name:?}"),
        &raw.harness,
        &raw.argv,
        raw.model.is_some(),
        raw.reasoning_effort.is_some(),
        false,
    )?;
    if raw.harness == "muse" && raw.mode == "headless" {
        validate_muse_headless_arguments(&name, &raw.argv)?;
    }
    let mut environment = Vec::new();
    for (key, value) in raw.env.0 {
        if !valid_env_name(&key) || key.contains('\0') {
            return Err(fail(format!(
                "profile {name:?} has an invalid environment variable name"
            )));
        }
        if secret_name(&key) {
            return Err(fail(format!(
                "profile {name:?} environment {key:?} appears secret; use the harness credential store"
            )));
        }
        if value.contains('\0') {
            return Err(fail(format!(
                "profile {name:?} environment values must be NUL-free strings"
            )));
        }
        environment.push(format!("{key}={value}"));
    }
    Ok(LaunchProfile {
        name,
        harness: raw.harness,
        mode: raw.mode,
        model: raw.model,
        reasoning_effort: raw.reasoning_effort,
        argv: raw.argv,
        environment,
    })
}

pub(crate) fn load_profiles(
    cwd: &Path,
    absent_ok: bool,
) -> Result<(PathBuf, BTreeMap<String, LaunchProfile>), AgentError> {
    let cwd = fs::canonicalize(cwd).map_err(|error| {
        fail(format!(
            "cwd is not a directory: {}: {error}",
            cwd.display()
        ))
    })?;
    let path = cwd.join(PROFILE_PATH);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && absent_ok => {
            return Ok((path, BTreeMap::new()));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(fail(format!(
                "profile config does not exist: {}",
                path.display()
            )));
        }
        Err(error) => {
            return Err(fail(format!(
                "cannot inspect profile config {}: {error}",
                path.display()
            )));
        }
    }
    let parent = fs::symlink_metadata(path.parent().expect("profile parent")).map_err(|error| {
        fail(format!(
            "cannot inspect profile config {}: {error}",
            path.display()
        ))
    })?;
    if parent.file_type().is_symlink() || !parent.is_dir() {
        return Err(fail(format!(
            "profile config parent must be a real directory: {}",
            path.parent().expect("profile parent").display()
        )));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|error| {
            fail(format!(
                "cannot open profile config {}: {error}",
                path.display()
            ))
        })?;
    let metadata = file.metadata().map_err(|error| {
        fail(format!(
            "cannot inspect profile config {}: {error}",
            path.display()
        ))
    })?;
    let uid = unsafe { libc::geteuid() };
    if parent.uid() != uid
        || parent.mode() & 0o077 != 0
        || !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(fail(format!(
            "profile config must be a same-user private single-link regular file: {}",
            path.display()
        )));
    }
    require_ignored(&cwd, &path)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            fail(format!(
                "cannot read profile config {}: {error}",
                path.display()
            ))
        })?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(fail(format!(
            "profile config exceeds {MAX_CONFIG_BYTES} bytes: {}",
            path.display()
        )));
    }
    let text = String::from_utf8(bytes).map_err(|error| {
        fail(format!(
            "cannot read profile config {}: {error}",
            path.display()
        ))
    })?;
    let document: Document = serde_json::from_str(&text).map_err(|error| {
        fail(format!(
            "cannot read profile config {}: {error}",
            path.display()
        ))
    })?;
    if document.schema != SCHEMA {
        return Err(fail(format!(
            "profile config must use schema {SCHEMA:?} and an object of profiles"
        )));
    }
    let profiles = document
        .profiles
        .0
        .into_iter()
        .map(|(name, profile)| validate(name.clone(), profile).map(|value| (name, value)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    Ok((path, profiles))
}

pub(crate) fn reasoning_arguments(
    harness: &str,
    effort: Option<&str>,
) -> Result<Vec<String>, AgentError> {
    let Some(effort) = effort else {
        return Ok(Vec::new());
    };
    if !matches!(
        effort,
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    ) {
        return Err(fail(format!("unsupported reasoning effort {effort:?}")));
    }
    match harness {
        "codex" => Ok(vec![
            "--config".to_owned(),
            format!("model_reasoning_effort={effort}"),
        ]),
        "claude" => Ok(vec!["--effort".to_owned(), effort.to_owned()]),
        "muse" => Ok(vec!["--reasoning-effort".to_owned(), effort.to_owned()]),
        _ => Err(fail(format!(
            "reasoning effort is not supported for harness {harness:?}"
        ))),
    }
}
