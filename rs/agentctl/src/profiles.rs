//! Strict project-local launch profiles.
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
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
use crate::error::AdapterError;
use crate::subagents::CloudLaunch;

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
    /// Agentcloud settings; present exactly when `harness` is `agentcloud`.
    pub(crate) cloud: Option<CloudLaunch>,
    /// `false` turns off the `created_by` and `managed_by` edges `start` records.
    pub(crate) record_edges: Option<bool>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    agentcloud: Option<CloudLaunch>,
    /// Agentcloud profiles are launchable only by this edition; the other edition lists them too.
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_edition: Option<&'static str>,
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
            agentcloud: self.cloud.clone(),
            requires_edition: self.cloud.as_ref().map(|_| "rust"),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: String,
    #[serde(default)]
    workspace: Option<Value>,
    #[serde(default)]
    wrkslots: Option<Value>,
    #[serde(default)]
    term_backend: Option<Value>,
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
    #[serde(default)]
    agentcloud: Option<CloudLaunch>,
    #[serde(default)]
    record_edges: Option<bool>,
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

fn valid_workspace(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.chars().any(char::is_control)
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
    let duplicate_resume = (matches!(harness, "codex" | "muse")
        && argv.iter().any(|value| value == "resume"))
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

/// How long one `git check-ignore` may run before it counts as no answer.
const IGNORE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// The waits between attempts when git does not answer and no earlier answer can stand in.
const IGNORE_CHECK_RETRIES: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];
/// After git fails to answer, how long the last answer it gave stands before git is asked again.
/// It doubles with each further failure, up to `IGNORE_CHECK_BACKOFF_MAX`.
const IGNORE_CHECK_BACKOFF_MIN: Duration = Duration::from_secs(30);
const IGNORE_CHECK_BACKOFF_MAX: Duration = Duration::from_secs(600);

/// How `require_ignored` asks git: the production values above, or a test's.
#[derive(Clone, Debug)]
struct IgnoreCheckPolicy {
    /// `None` runs the verified system Git; a test names its own stand-in.
    git: Option<PathBuf>,
    timeout: Duration,
    retries: Vec<Duration>,
    backoff_min: Duration,
    backoff_max: Duration,
}

impl Default for IgnoreCheckPolicy {
    fn default() -> Self {
        Self {
            git: None,
            timeout: IGNORE_CHECK_TIMEOUT,
            retries: IGNORE_CHECK_RETRIES.to_vec(),
            backoff_min: IGNORE_CHECK_BACKOFF_MIN,
            backoff_max: IGNORE_CHECK_BACKOFF_MAX,
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_IGNORE_CHECK: std::cell::RefCell<Option<IgnoreCheckPolicy>> =
        const { std::cell::RefCell::new(None) };
}

/// A stand-in for Git in tests, and the policy that runs it on this thread.
#[cfg(test)]
pub(crate) mod test_git {
    use super::{IgnoreCheckPolicy, TEST_IGNORE_CHECK};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// A Git that runs the system Git, except that while its `hang` file exists it sleeps for
    /// 30 s instead, past any timeout a test sets.
    pub(crate) struct HangingGit {
        pub(crate) executable: PathBuf,
        pub(crate) hang: PathBuf,
    }

    impl HangingGit {
        /// Write the stand-in into `directory`.
        pub(crate) fn new(directory: &Path) -> Self {
            let executable = directory.join("hanging-git");
            let hang = directory.join("git-hangs");
            fs::write(
                &executable,
                format!(
                    "#!/bin/sh\nif [ -e '{}' ]; then exec sleep 30; fi\nexec {} \"$@\"\n",
                    hang.display(),
                    super::SYSTEM_GIT
                ),
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            Self { executable, hang }
        }

        pub(crate) fn hang(&self, hang: bool) {
            if hang {
                fs::write(&self.hang, b"").unwrap();
            } else {
                let _ = fs::remove_file(&self.hang);
            }
        }

        /// Ask this Git on this thread until the guard drops, timing a check out after
        /// `timeout`, retrying once after 50 ms with no earlier answer, and backing off from
        /// `backoff` up to twice that.
        pub(crate) fn install(&self, timeout: Duration, backoff: Duration) -> Installed {
            TEST_IGNORE_CHECK.with(|policy| {
                *policy.borrow_mut() = Some(IgnoreCheckPolicy {
                    git: Some(self.executable.clone()),
                    timeout,
                    retries: vec![Duration::from_millis(50)],
                    backoff_min: backoff,
                    backoff_max: backoff * 2,
                });
            });
            Installed
        }
    }

    /// Restores the production policy on drop.
    pub(crate) struct Installed;

    impl Drop for Installed {
        fn drop(&mut self) {
            TEST_IGNORE_CHECK.with(|policy| *policy.borrow_mut() = None);
        }
    }
}

fn ignore_check_policy() -> IgnoreCheckPolicy {
    #[cfg(test)]
    if let Some(policy) = TEST_IGNORE_CHECK.with(|policy| policy.borrow().clone()) {
        return policy;
    }
    IgnoreCheckPolicy::default()
}

/// The checks of one profile config that git did not answer since it last did, as `chat status`
/// reports them (#581 bridge-git-timeout-fatal).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IgnoreCheckFailure {
    /// When the first of these checks failed, in Unix milliseconds.
    pub(crate) since_millis: u64,
    pub(crate) failures: u64,
    pub(crate) last_at_millis: u64,
    pub(crate) last_error: String,
}

/// What this process last learned from git about one profile config.
#[derive(Debug, Default)]
struct IgnoreCheckState {
    /// Git last said the config is ignored.
    ignored: bool,
    /// Set from git's first failure to answer until it answers again.
    failure: Option<IgnoreCheckFailure>,
    /// While git is failing, it is not asked again before this; the last answer stands.
    retry_at: Option<Instant>,
    backoff: Duration,
}

type IgnoreCheckKey = (PathBuf, PathBuf);

static IGNORE_CHECKS: std::sync::Mutex<BTreeMap<IgnoreCheckKey, IgnoreCheckState>> =
    std::sync::Mutex::new(BTreeMap::new());

fn ignore_checks() -> std::sync::MutexGuard<'static, BTreeMap<IgnoreCheckKey, IgnoreCheckState>> {
    IGNORE_CHECKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The checks git has not answered for the profile config beside `registry`, while it is not
/// answering and its last answer stands in. `None` when it answered the last check, or when this
/// process has not checked that config.
pub(crate) fn ignore_check_failure(registry: &Path) -> Option<IgnoreCheckFailure> {
    let root = registry.parent()?;
    ignore_checks()
        .get(&(root.to_path_buf(), PathBuf::from(PROFILE_PATH)))
        .and_then(|state| state.failure.clone())
}

fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Log one line as `chat run` logs, so a long-lived service shows when git stops and starts
/// answering; a short command shows it on stderr.
fn ignore_check_log(line: impl fmt::Display) {
    crate::chat_service::service_log(line);
}

/// Refuse unless Git ignores the profile config at `path` below `cwd`.
///
/// A check git does not answer, because it timed out, could not be started, or was killed, is
/// not an answer that the config stopped being ignored. When git said it was ignored earlier in
/// this process, that answer stands, git is asked again only after a backoff (30 s, doubling to
/// 10 min), and the failure is logged once and reported by [`ignore_check_failure`], so a slow
/// git on a loaded host never stops a long-lived `chat run` (#581 bridge-git-timeout-fatal).
/// With no earlier answer the check is tried three times before it fails. Git saying the config
/// is not ignored always refuses at once.
fn require_ignored(cwd: &Path, path: &Path) -> Result<(), AgentError> {
    let relative = path
        .strip_prefix(cwd)
        .map_err(|_| fail("profile config must remain below the selected working directory"))?;
    let policy = ignore_check_policy();
    let key = (cwd.to_path_buf(), relative.to_path_buf());
    if ignore_checks()
        .get(&key)
        .is_some_and(|state| state.ignored && state.retry_at.is_some_and(|at| Instant::now() < at))
    {
        return Ok(());
    }
    let git = match &policy.git {
        Some(git) => git.clone(),
        None => verified_system_git()?,
    };
    let mut retries = policy.retries.iter();
    let answer = loop {
        match ask_check_ignore(&git, cwd, relative, policy.timeout) {
            Ok(ignored) => break Ok(ignored),
            Err(error) => {
                let stands_in = ignore_checks().get(&key).is_some_and(|state| state.ignored);
                match retries.next() {
                    Some(wait) if !stands_in => thread::sleep(*wait),
                    _ => break Err(error),
                }
            }
        }
    };
    let mut checks = ignore_checks();
    match answer {
        Ok(true) => {
            let state = checks.entry(key).or_default();
            state.ignored = true;
            state.retry_at = None;
            state.backoff = Duration::ZERO;
            if let Some(failure) = state.failure.take() {
                ignore_check_log(format_args!(
                    "agentctl: git check-ignore answers again for {} after {} failed checks",
                    path.display(),
                    failure.failures
                ));
            }
            Ok(())
        }
        Ok(false) => {
            checks.remove(&key);
            Err(fail(format!(
                "profile config is not ignored by Git: {}; add .agentctl/ to .gitignore",
                relative.display()
            )))
        }
        Err(error) => {
            let Some(state) = checks.get_mut(&key).filter(|state| state.ignored) else {
                let attempts = policy.retries.len() + 1;
                return Err(AgentError::Client(AdapterError::unavailable(format!(
                    "cannot verify that profile config is ignored: {error} ({attempts} attempts)"
                ))));
            };
            state.backoff = if state.backoff.is_zero() {
                policy.backoff_min
            } else {
                (state.backoff * 2).min(policy.backoff_max)
            };
            state.retry_at = Some(Instant::now() + state.backoff);
            let now = unix_millis_now();
            match state.failure.as_mut() {
                Some(failure) => {
                    failure.failures = failure.failures.saturating_add(1);
                    failure.last_at_millis = now.max(failure.since_millis);
                    failure.last_error = error;
                }
                None => {
                    ignore_check_log(format_args!(
                        "agentctl: warning: {error}; keeping the earlier answer that {} is \
                         ignored and asking git again in {}",
                        path.display(),
                        seconds_or_millis(state.backoff)
                    ));
                    state.failure = Some(IgnoreCheckFailure {
                        since_millis: now,
                        failures: 1,
                        last_at_millis: now,
                        last_error: error,
                    });
                }
            }
            Ok(())
        }
    }
}

fn verified_system_git() -> Result<PathBuf, AgentError> {
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
    Ok(git)
}

/// Ask `git check-ignore` whether `relative` below `cwd` is ignored: `Ok(true)` when it exits 0,
/// `Ok(false)` for any other exit status, and `Err` when it gives no answer: it cannot be
/// started or waited for, runs past `timeout`, or is killed by a signal.
fn ask_check_ignore(
    git: &Path,
    cwd: &Path,
    relative: &Path,
    timeout: Duration,
) -> Result<bool, String> {
    let mut child = Command::new(git)
        .args(["-C"])
        .arg(cwd)
        .args(["check-ignore", "--quiet", "--"])
        .arg(relative)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start git check-ignore: {error}"))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for git check-ignore: {error}"));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "git check-ignore timed out after {}",
                seconds_or_millis(timeout)
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    match status.code() {
        Some(0) => Ok(true),
        Some(_) => Ok(false),
        None => Err(format!("git check-ignore was killed: {status}")),
    }
}

fn seconds_or_millis(duration: Duration) -> String {
    if duration.subsec_millis() == 0 {
        format!("{} seconds", duration.as_secs())
    } else {
        format!("{} ms", duration.as_millis())
    }
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

fn validate(name: String, mut raw: RawProfile) -> Result<LaunchProfile, AgentError> {
    if !valid_name(&name) {
        return Err(fail(format!(
            "invalid profile name {name:?}; use lowercase letters, digits, and hyphens"
        )));
    }
    if !matches!(
        raw.harness.as_str(),
        "codex" | "claude" | "muse" | "agy" | "agentcloud"
    ) {
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
        matches!(
            raw.harness.as_str(),
            "codex" | "claude" | "muse" | "agentcloud"
        )
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
    if raw.harness == "agentcloud" {
        let label = format!("profile {name:?}");
        crate::subagents::validate_cloud_create_arguments(&label, &raw.argv)?;
        raw.agentcloud
            .get_or_insert_with(CloudLaunch::default)
            .validate(&label)?;
    } else if raw.agentcloud.is_some() {
        return Err(fail(format!(
            "profile {name:?} has an agentcloud block but harness {:?}; use harness \"agentcloud\" or remove the block",
            raw.harness
        )));
    } else {
        validate_raw_harness_arguments(
            &format!("profile {name:?}"),
            &raw.harness,
            &raw.argv,
            raw.model.is_some(),
            raw.reasoning_effort.is_some(),
            false,
        )?;
    }
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
        cloud: raw.agentcloud,
        record_edges: raw.record_edges,
    })
}

/// A loaded project configuration: the configuration file's path, its launch profiles by name,
/// and the Herdr workspace the project pins, if any.
pub(crate) type ProjectConfiguration = (PathBuf, BTreeMap<String, LaunchProfile>, Option<String>);

pub(crate) fn load_configuration(
    cwd: &Path,
    absent_ok: bool,
) -> Result<ProjectConfiguration, AgentError> {
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
            return Ok((path, BTreeMap::new(), None));
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
    let workspace = match document.workspace {
        None | Some(Value::Null) => None,
        Some(Value::String(workspace)) if valid_workspace(&workspace) => Some(workspace),
        Some(_) => {
            return Err(fail(
                "profile config workspace must be 1-128 bytes, have no surrounding whitespace, and contain no control characters",
            ))
        }
    };
    let profiles = document
        .profiles
        .0
        .into_iter()
        .map(|(name, profile)| validate(name.clone(), profile).map(|value| (name, value)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    wrkslots_value(document.wrkslots.as_ref())?;
    backend_value(document.term_backend.as_ref())?;
    Ok((path, profiles, workspace))
}

/// The optional `wrkslots` key: an absolute path to the slot manager command.
fn wrkslots_value(value: Option<&Value>) -> Result<Option<String>, AgentError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(path))
            if !path.is_empty() && !path.contains('\0') && Path::new(path).is_absolute() =>
        {
            Ok(Some(path.clone()))
        }
        Some(_) => Err(fail(
            "profile config wrkslots must be an absolute path to the wrkslots command",
        )),
    }
}

pub(crate) fn load_profiles(
    cwd: &Path,
    absent_ok: bool,
) -> Result<(PathBuf, BTreeMap<String, LaunchProfile>), AgentError> {
    let (path, profiles, _) = load_configuration(cwd, absent_ok)?;
    Ok((path, profiles))
}

/// Choose the directory whose `.agentctl/profiles.json` governs a start in `cwd`.
///
/// `cwd` wins when it has a profile file. Otherwise a conventional registry's project (the
/// parent of a registry directory named `.agentctl`) is used, the same project workspace policy
/// is read from, so an agent started in a worktree outside that project still finds the
/// project's profiles. When neither has one, the refusal names both paths, or with `absent_ok`
/// `cwd` is returned.
pub(crate) fn configuration_root(
    cwd: &Path,
    registry: &Path,
    absent_ok: bool,
) -> Result<PathBuf, AgentError> {
    let root = fs::canonicalize(cwd).map_err(|error| {
        fail(format!(
            "cwd is not a directory: {}: {error}",
            cwd.display()
        ))
    })?;
    let mut candidates = vec![root.clone()];
    if registry.file_name().and_then(|name| name.to_str()) == Some(".agentctl") {
        let registry = std::path::absolute(registry).map_err(|error| {
            fail(format!(
                "cannot resolve agent registry {}: {error}",
                registry.display()
            ))
        })?;
        if let Some(parent) = registry.parent() {
            let parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            if parent != root {
                candidates.push(parent);
            }
        }
    }
    for candidate in &candidates {
        let path = candidate.join(PROFILE_PATH);
        match fs::symlink_metadata(&path) {
            Ok(_) => return Ok(candidate.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(fail(format!(
                    "cannot inspect profile config {}: {error}",
                    path.display()
                )))
            }
        }
    }
    if candidates.len() > 1 && !absent_ok {
        return Err(fail(format!(
            "profile config does not exist: {} (nor beside the registry: {})",
            candidates[0].join(PROFILE_PATH).display(),
            candidates[1].join(PROFILE_PATH).display()
        )));
    }
    Ok(root)
}

/// Read the workspace policy belonging to a conventional project registry.
///
/// Nonstandard registry directories use the default behavior: they do
/// not acquire policy from an unrelated `.agentctl` directory beside them.
pub(crate) fn workspace_for_registry(registry: &Path) -> Result<Option<String>, AgentError> {
    let Some(object) = registry_policy(registry)? else {
        return Ok(None);
    };
    match object.get("workspace") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if valid_workspace(value) => Ok(Some(value.clone())),
        Some(_) => Err(fail(
            "profile config workspace must be 1-128 bytes, have no surrounding whitespace, and contain no control characters",
        )),
    }
}

/// The optional `term_backend` key: `"herdr"` or `"tmux"`.
fn backend_value(value: Option<&Value>) -> Result<Option<term_backend::BackendKind>, AgentError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => term_backend::BackendKind::parse(raw)
            .map(Some)
            .map_err(|_| {
                fail(format!(
                    "profile config term_backend must be \"herdr\" or \"tmux\", got {raw:?}"
                ))
            }),
        Some(_) => Err(fail(
            "profile config term_backend must be \"herdr\" or \"tmux\"",
        )),
    }
}

/// Read only the configured terminal backend for a conventional project registry.
pub(crate) fn backend_for_registry(
    registry: &Path,
) -> Result<Option<term_backend::BackendKind>, AgentError> {
    match registry_policy(registry)? {
        Some(object) => backend_value(object.get("term_backend")),
        None => Ok(None),
    }
}

/// Read only the configured wrkslots command for a conventional project registry.
pub(crate) fn wrkslots_for_registry(registry: &Path) -> Result<Option<String>, AgentError> {
    match registry_policy(registry)? {
        Some(object) => wrkslots_value(object.get("wrkslots")),
        None => Ok(None),
    }
}

/// The raw project configuration object beside a conventional registry, if any.
fn registry_policy(registry: &Path) -> Result<Option<BTreeMap<String, Value>>, AgentError> {
    if registry.file_name().and_then(|name| name.to_str()) != Some(".agentctl") {
        return Ok(None);
    }
    let root = registry
        .parent()
        .ok_or_else(|| fail("agent registry has no project parent"))?;
    let path = root.join(PROFILE_PATH);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(fail(format!(
                "cannot inspect profile config {}: {error}",
                path.display()
            )))
        }
    }
    let config_parent = path.parent().expect("profile config parent");
    let parent = fs::symlink_metadata(config_parent).map_err(|error| {
        fail(format!(
            "cannot inspect profile config {}: {error}",
            path.display()
        ))
    })?;
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
    if parent.file_type().is_symlink()
        || !parent.is_dir()
        || parent.uid() != uid
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
    require_ignored(root, &path)?;
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
    let UniqueMap(object): UniqueMap<Value> = serde_json::from_slice(&bytes).map_err(|error| {
        fail(format!(
            "cannot read profile config {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(object))
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
    crate::harness::adapter(harness)
        .filter(|adapter| adapter.structured_launch())
        .and_then(|adapter| adapter.reasoning_arguments(effort))
        .ok_or_else(|| {
            fail(format!(
                "reasoning effort is not supported for harness {harness:?}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn profile_resume_rejects_duplicate_native_subcommands_but_preserves_raw_only_policy() {
        let arguments = vec!["resume".to_owned(), "raw-conversation".to_owned()];
        for harness in ["codex", "muse"] {
            assert!(validate_structured_harness_argument_conflicts(
                "launch", harness, &arguments, false, false, false
            )
            .is_ok());
            let error = validate_structured_harness_argument_conflicts(
                "launch", harness, &arguments, false, false, true,
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                "launch cannot repeat the structured resume selector in raw arguments"
            );
            let profile = parse(serde_json::json!({
                "harness": harness,
                "mode": "interactive",
                "argv": arguments,
            }))
            .expect("raw-only profile remains valid");
            assert_eq!(profile.argv, arguments);
        }
    }

    #[test]
    fn workspace_labels_are_bounded_literal_non_control_text() {
        assert!(valid_workspace("project-agents"));
        assert!(valid_workspace("project ☃"));
        for invalid in ["", " leading", "trailing ", "line\nfeed", "tab\tlabel"] {
            assert!(!valid_workspace(invalid), "accepted {invalid:?}");
        }
        assert!(!valid_workspace(&"x".repeat(129)));
        assert!(valid_workspace(&"x".repeat(128)));
    }

    #[test]
    fn configuration_exposes_workspace_only_to_its_project_registry() {
        let root = std::env::temp_dir().join(format!(
            "agentctl-profiles-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        assert!(Command::new(SYSTEM_GIT)
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        fs::write(root.join(".gitignore"), ".agentctl/\n").unwrap();
        let directory = root.join(".agentctl");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("profiles.json");
        fs::write(
            &path,
            r#"{"schema":"agentctl-profiles/v1","workspace":"project-agents","profiles":{}}"#,
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let (_, profiles, workspace) = load_configuration(&root, false).unwrap();
        assert!(profiles.is_empty());
        assert_eq!(workspace.as_deref(), Some("project-agents"));
        assert_eq!(
            workspace_for_registry(&directory).unwrap().as_deref(),
            Some("project-agents")
        );
        assert_eq!(
            workspace_for_registry(&root.join("unrelated-registry")).unwrap(),
            None
        );
        fs::write(
            &path,
            r#"{"schema":"agentctl-profiles/v1","workspace":"first","workspace":"second","profiles":{"broken":{"harness":"codex"}}}"#,
        )
        .unwrap();
        let error = workspace_for_registry(&directory).unwrap_err();
        assert!(error.to_string().contains("duplicate key \"workspace\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_git_that_stops_answering_keeps_the_last_answer_and_backs_off() {
        // #581 bridge-git-timeout-fatal: a long-lived chat run exited when one check of its
        // registry's profile config timed out on a loaded host.
        let root = std::env::temp_dir().join(format!(
            "agentctl-profiles-hang-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        assert!(Command::new(SYSTEM_GIT)
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        fs::write(root.join(".gitignore"), ".agentctl/\n").unwrap();
        let registry = root.join(".agentctl");
        fs::create_dir(&registry).unwrap();
        fs::set_permissions(&registry, fs::Permissions::from_mode(0o700)).unwrap();
        let path = registry.join("profiles.json");
        fs::write(
            &path,
            r#"{"schema":"agentctl-profiles/v1","workspace":"project-agents","profiles":{}}"#,
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let git = test_git::HangingGit::new(&root);
        let timeout = Duration::from_millis(300);
        let backoff = Duration::from_millis(400);
        let _installed = git.install(timeout, backoff);
        let workspace = || workspace_for_registry(&registry);

        assert_eq!(workspace().unwrap().as_deref(), Some("project-agents"));
        assert_eq!(ignore_check_failure(&registry), None);

        // Git stops answering: the earlier answer stands, and the failure is reported.
        git.hang(true);
        let asked = Instant::now();
        assert_eq!(workspace().unwrap().as_deref(), Some("project-agents"));
        assert!(asked.elapsed() >= timeout, "{:?}", asked.elapsed());
        let failure = ignore_check_failure(&registry).expect("the failure is reported");
        assert_eq!(failure.failures, 1);
        assert!(
            failure.last_error.contains("timed out after 300 ms"),
            "{}",
            failure.last_error
        );
        // Within the backoff git is not asked: the answer comes at once.
        let asked = Instant::now();
        assert_eq!(workspace().unwrap().as_deref(), Some("project-agents"));
        assert!(asked.elapsed() < timeout, "{:?}", asked.elapsed());
        assert_eq!(ignore_check_failure(&registry).unwrap().failures, 1);
        // After it, git is asked again, fails again, and the backoff doubles.
        thread::sleep(backoff);
        assert_eq!(workspace().unwrap().as_deref(), Some("project-agents"));
        let failure = ignore_check_failure(&registry).unwrap();
        assert_eq!(failure.failures, 2);
        thread::sleep(backoff);
        let asked = Instant::now();
        workspace().unwrap();
        assert!(
            asked.elapsed() < timeout,
            "the doubled backoff had not run out"
        );

        // Git answers again once the backoff runs out, and the failure clears.
        git.hang(false);
        thread::sleep(backoff * 2);
        assert_eq!(workspace().unwrap().as_deref(), Some("project-agents"));
        assert_eq!(ignore_check_failure(&registry), None);

        // Git saying the config is not ignored refuses at once and forgets the earlier answer...
        fs::remove_file(root.join(".gitignore")).unwrap();
        let error = workspace().unwrap_err();
        assert!(
            error.to_string().contains("is not ignored by Git"),
            "{error}"
        );
        assert!(matches!(error, AgentError::Delivery(_)), "{error:?}");
        // ...so a check git does not answer then has nothing to stand in, and fails after its
        // retry as an unavailable runtime, which chat run's owner loop starts again after.
        git.hang(true);
        let error = workspace().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("timed out after 300 ms (2 attempts)"),
            "{error}"
        );
        assert!(
            matches!(&error, AgentError::Client(client) if client.kind() == crate::error::AdapterErrorKind::Unavailable),
            "{error:?}"
        );
        assert_eq!(ignore_check_failure(&registry), None);
        git.hang(false);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worktree_without_profiles_uses_the_registry_project_profiles() {
        let base = std::env::temp_dir().join(format!(
            "agentctl-profile-root-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let project = base.join("project");
        let slot = base.join("worktrees").join("slot-a");
        let bare = base.join("bare");
        for directory in [&project, &slot, &bare] {
            fs::create_dir_all(directory).unwrap();
        }
        assert!(Command::new(SYSTEM_GIT)
            .args(["init", "-q"])
            .arg(&project)
            .status()
            .unwrap()
            .success());
        fs::write(project.join(".gitignore"), ".agentctl/\n").unwrap();
        let registry = project.join(".agentctl");
        fs::create_dir(&registry).unwrap();
        fs::set_permissions(&registry, fs::Permissions::from_mode(0o700)).unwrap();
        let path = registry.join("profiles.json");
        fs::write(
            &path,
            r#"{"schema":"agentctl-profiles/v1","profiles":{"local":{"harness":"codex","mode":"interactive"}}}"#,
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let project = fs::canonicalize(&project).unwrap();

        let root = configuration_root(&slot, &registry, false).unwrap();
        assert_eq!(root, project);
        let (_, profiles) = load_profiles(&root, false).unwrap();
        assert!(profiles.contains_key("local"));
        // The working directory's own profiles win.
        assert_eq!(
            configuration_root(&project, &registry, false).unwrap(),
            project
        );
        // Neither has profiles: the refusal names both paths.
        let unconfigured = base.join("unconfigured").join(".agentctl");
        let error = configuration_root(&bare, &unconfigured, false).unwrap_err();
        assert!(
            error.to_string().contains("nor beside the registry"),
            "{error}"
        );
        let bare = fs::canonicalize(&bare).unwrap();
        assert_eq!(
            configuration_root(&bare, &unconfigured, true).unwrap(),
            bare
        );
        // A registry not named .agentctl lends no profiles, as for workspace policy.
        assert_eq!(
            configuration_root(&bare, &project.join("registry"), true).unwrap(),
            bare
        );
        fs::remove_dir_all(base).unwrap();
    }

    fn parse(profile: serde_json::Value) -> Result<LaunchProfile, AgentError> {
        validate(
            "sub-cloud".to_owned(),
            serde_json::from_value(profile).expect("profile shape"),
        )
    }

    #[test]
    fn agentcloud_profiles_carry_structured_launch_settings() {
        let profile = parse(serde_json::json!({
            "harness": "agentcloud",
            "mode": "interactive",
            "model": "provider-model-id",
            "reasoning_effort": "high",
            "argv": ["--skill=builder", "--narration"],
            "env": {"TAB_SETTING": "1"},
            "agentcloud": {
                "harness": "claude-code",
                "provision": true,
                "envspec": "monorepo",
                "purpose": "sub-cloud worker"
            }
        }))
        .expect("valid agentcloud profile");
        let cloud = profile.cloud.clone().expect("cloud settings");
        assert_eq!(cloud.harness.as_deref(), Some("claude-code"));
        assert!(cloud.provision);
        assert_eq!(cloud.envspec.as_deref(), Some("monorepo"));
        let public = serde_json::to_value(profile.public()).expect("public profile");
        assert_eq!(public["agentcloud"]["envspec"], "monorepo");
        assert_eq!(public["requires_edition"], "rust");
        assert_eq!(public["environment"], serde_json::json!(["TAB_SETTING"]));

        let templated = parse(serde_json::json!({
            "harness": "agentcloud",
            "mode": "interactive",
            "agentcloud": {"node_id": "{fqdn}", "workspace": "{cwd}", "title": "{name}-{host}"}
        }))
        .expect("placeholders are valid in node_id, workspace, and title");
        assert_eq!(
            templated.public().agentcloud.unwrap().title.as_deref(),
            Some("{name}-{host}")
        );
        assert!(parse(serde_json::json!({
            "harness": "agentcloud",
            "mode": "interactive",
            "agentcloud": {"title": "{user}"}
        }))
        .is_err());

        let minimal = parse(serde_json::json!({"harness": "agentcloud", "mode": "interactive"}))
            .expect("server defaults are valid");
        assert_eq!(minimal.cloud, Some(CloudLaunch::default()));
    }

    #[test]
    fn agentcloud_profiles_refuse_ambiguous_or_misplaced_settings() {
        for (profile, expected) in [
            (
                serde_json::json!({"harness": "agentcloud", "mode": "headless"}),
                "harness/mode combination",
            ),
            (
                serde_json::json!({"harness": "claude", "mode": "interactive", "agentcloud": {}}),
                "agentcloud block",
            ),
            (
                serde_json::json!({"harness": "agentcloud", "mode": "interactive", "agentcloud": {"envspec": "monorepo"}}),
                "without provision",
            ),
            (
                serde_json::json!({"harness": "agentcloud", "mode": "interactive", "argv": ["--title=other"]}),
                "--title",
            ),
            (
                serde_json::json!({"harness": "agentcloud", "mode": "interactive", "argv": ["--skill", "builder"]}),
                "--option=value",
            ),
            (
                serde_json::json!({"harness": "agentcloud", "mode": "interactive", "argv": ["--token=abc"]}),
                "secret",
            ),
        ] {
            let message = parse(profile).expect_err("refused profile").to_string();
            assert!(
                message.contains(expected),
                "{expected:?} not in {message:?}"
            );
        }
        assert!(serde_json::from_value::<RawProfile>(serde_json::json!({
            "harness": "agentcloud",
            "mode": "interactive",
            "agentcloud": {"provison": true}
        }))
        .is_err());
    }
}
