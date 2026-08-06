//! Serialized pane execution with byte-preserving spool collection.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use fs2::FileExt;
use serde_json::{json, Value};

use crate::allowlist::Admission;
use crate::client::HerdrApi;
use crate::config::Config;
use crate::error::{HerdrRunError, Result};
use crate::readiness::{assess, infer_prompt_tail, Readiness};
use crate::session::Target;
use crate::state::{open_lock_file, pane_lock_path};
use crate::timefmt;

/// Files holding one run's command, byte streams, and completion status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpoolPaths {
    /// Exclusive directory allocated to this run.
    pub directory: PathBuf,
    /// Raw standard-output file.
    pub stdout: PathBuf,
    /// Raw standard-error file.
    pub stderr: PathBuf,
    /// Completion marker containing the wrapped command's exit code.
    pub exit_code: PathBuf,
}

/// One completed command and the evidence used to launch it.
#[derive(Clone, Debug, PartialEq)]
pub struct RunResult {
    /// Wrapped command exit code.
    pub exit_code: i32,
    /// Byte-exact standard output.
    pub stdout: Vec<u8>,
    /// Byte-exact standard error.
    pub stderr: Vec<u8>,
    /// Unique human-readable spool identifier.
    pub run_id: String,
    /// Paths allocated to this result.
    pub spool: SpoolPaths,
    /// Live-validated destination pane.
    pub target: Target,
    /// Two-signal readiness evidence.
    pub readiness: Readiness,
    /// Elapsed collection time after submission.
    pub duration_seconds: f64,
}

impl RunResult {
    /// Decode stdout for human-readable JSON, replacing invalid UTF-8 sequences.
    #[must_use]
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Decode stderr for human-readable JSON, replacing invalid UTF-8 sequences.
    #[must_use]
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Resolve the spool paths for `run_id` without creating them.
#[must_use]
pub fn spool_paths(config: &Config, run_id: &str) -> SpoolPaths {
    let root = spool_root(config);
    let directory = root.join("runs").join(run_id);
    SpoolPaths {
        stdout: directory.join("stdout"),
        stderr: directory.join("stderr"),
        exit_code: directory.join("exit_code"),
        directory,
    }
}

fn spool_root(config: &Config) -> PathBuf {
    let root = Path::new(&config.spool_dir);
    if root.is_absolute() {
        root.to_path_buf()
    } else {
        Path::new(&config.project_root).join(root)
    }
}

fn ensure_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn create_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(contents)
}

/// Build a readable base run ID from UTC time, agent, and process ID.
#[must_use]
pub fn make_run_id(agent: &str, now: SystemTime, pid: u32) -> String {
    let safe_agent: String = agent
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let safe_agent = if safe_agent.is_empty() {
        "agent"
    } else {
        &safe_agent
    };
    format!("{}-{safe_agent}-{pid}", timefmt::compact_utc(now))
}

fn allocate_spool(config: &Config, base_run_id: &str) -> Result<(String, SpoolPaths)> {
    let root = spool_root(config);
    let runs = root.join("runs");
    ensure_private_directory(&root).map_err(|error| {
        HerdrRunError::unavailable(format!(
            "cannot create private run spool root {}: {error}",
            root.display()
        ))
    })?;
    ensure_private_directory(&runs).map_err(|error| {
        HerdrRunError::unavailable(format!(
            "cannot create private run spool parent {}: {error}",
            runs.display()
        ))
    })?;
    for collision in 0..10_000_u32 {
        let run_id = if collision == 0 {
            base_run_id.to_owned()
        } else {
            format!("{base_run_id}-{collision}")
        };
        let spool = spool_paths(config, &run_id);
        let create = fs::DirBuilder::new().mode(0o700).create(&spool.directory);
        match create {
            Ok(()) => {
                fs::set_permissions(&spool.directory, fs::Permissions::from_mode(0o700)).map_err(
                    |error| {
                        HerdrRunError::unavailable(format!(
                            "cannot set private mode on run spool {}: {error}",
                            spool.directory.display()
                        ))
                    },
                )?;
                return Ok((run_id, spool));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(HerdrRunError::unavailable(format!(
                    "cannot create run spool {}: {error}",
                    spool.directory.display()
                )));
            }
        }
    }
    Err(HerdrRunError::unavailable(format!(
        "could not allocate a unique run spool after 10000 collisions for {base_run_id}"
    )))
}

/// Poll until readiness holds for two consecutive samples.
pub fn wait_ready<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    target: &Target,
    prompt_tail: Option<&str>,
    timeout: f64,
) -> Result<Readiness> {
    wait_ready_with(
        client,
        config,
        target,
        prompt_tail,
        timeout,
        Duration::from_millis(250),
        2,
    )
}

fn wait_ready_with<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    target: &Target,
    prompt_tail: Option<&str>,
    timeout: f64,
    poll_interval: Duration,
    required_consecutive: usize,
) -> Result<Readiness> {
    let deadline = Instant::now().checked_add(seconds_duration(timeout));
    let mut consecutive = 0;
    let mut readings = 0;
    let last = loop {
        let reading = assess(client, &target.pane_id, config, prompt_tail, 4)?;
        readings += 1;
        if reading.ready {
            consecutive += 1;
            if consecutive >= required_consecutive {
                return Ok(reading);
            }
        } else {
            consecutive = 0;
        }
        if readings >= required_consecutive && deadline.is_some_and(|value| Instant::now() >= value)
        {
            break reading;
        }
        thread::sleep(poll_interval);
    };
    if last.ready {
        return Err(HerdrRunError::busy(format!(
            "pane {} ({}) looked idle but not for {required_consecutive} consecutive checks within {}s: {}",
            target.pane_id,
            target.tab_label,
            number_text(timeout),
            last.describe()
        )));
    }
    Err(HerdrRunError::busy(format!(
        "pane {} ({}) is not ready after {}s: {}. Nothing was executed.",
        target.pane_id,
        target.tab_label,
        number_text(timeout),
        last.describe()
    )))
}

fn acquire_pane_lock(target: &Target, timeout: f64) -> Result<File> {
    let path = pane_lock_path(&target.pane_id)?;
    let file = open_lock_file(&path)?;
    let deadline = Instant::now().checked_add(seconds_duration(timeout));
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if deadline.is_some_and(|value| Instant::now() >= value) {
                    return Err(HerdrRunError::busy(format!(
                        "pane {} ({}) is already reserved by another herdr-run. Nothing was executed.",
                        target.pane_id, target.tab_label
                    )));
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                return Err(HerdrRunError::unavailable(format!(
                    "cannot lock pane {}: {error}",
                    target.pane_id
                )));
            }
        }
    }
}

fn contains_terminal_control(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(u32::from(character), 0x00..=0x1f | 0x7f..=0x9f))
}

fn quote_word(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    }) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Render the line typed into the interactive pane.
///
/// The outer command is accepted by common interactive shells, while all grouping and redirection
/// is deliberately parsed by `sh`.
pub fn build_shell_command(
    admission: &Admission,
    spool: &SpoolPaths,
    cwd: &Path,
) -> Result<String> {
    let values = [
        ("working directory", cwd),
        ("stdout spool path", spool.stdout.as_path()),
        ("stderr spool path", spool.stderr.as_path()),
        ("exit-code spool path", spool.exit_code.as_path()),
    ];
    for (description, value) in values {
        let text = value.to_str().ok_or_else(|| {
            HerdrRunError::unavailable(format!("{description} is not valid UTF-8"))
        })?;
        if contains_terminal_control(text) {
            return Err(HerdrRunError::unavailable(format!(
                "{description} contains terminal control characters"
            )));
        }
    }
    let inner = format!(
        "umask 077; {{ cd {} && {} ; }} >{} 2>{}; printf '%s\\n' \"$?\" >{}",
        quote_word(cwd.to_str().expect("validated UTF-8")),
        admission.rendered(),
        quote_word(spool.stdout.to_str().expect("validated UTF-8")),
        quote_word(spool.stderr.to_str().expect("validated UTF-8")),
        quote_word(spool.exit_code.to_str().expect("validated UTF-8"))
    );
    Ok(format!("command sh -c {}", quote_word(&inner)))
}

/// Execute one admitted command while holding the pane's inter-process lock.
#[allow(clippy::too_many_arguments)]
pub fn execute<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    target: &Target,
    admission: &Admission,
    agent: &str,
    cwd: &Path,
    ready_timeout: f64,
    timeout: f64,
) -> Result<RunResult> {
    let lock_started = Instant::now();
    let lock = acquire_pane_lock(target, ready_timeout)?;
    let elapsed = lock_started.elapsed().as_secs_f64();
    let remaining_ready = (ready_timeout - elapsed).max(0.0);
    let prompt_tail = infer_prompt_tail(config, None);
    let readiness = wait_ready(
        client,
        config,
        target,
        prompt_tail.as_deref(),
        remaining_ready,
    )?;

    let base_run_id = make_run_id(agent, SystemTime::now(), std::process::id());
    let (run_id, spool) = allocate_spool(config, &base_run_id)?;
    create_private_file(&spool.stdout, &[]).map_err(|error| spool_error(&spool, error))?;
    create_private_file(&spool.stderr, &[]).map_err(|error| spool_error(&spool, error))?;
    create_private_file(
        &spool.directory.join("command"),
        format!("{}\n", admission.rendered()).as_bytes(),
    )
    .map_err(|error| spool_error(&spool, error))?;

    let started = Instant::now();
    client.run(
        &target.pane_id,
        &build_shell_command(admission, &spool, cwd)?,
    )?;
    let deadline = started.checked_add(seconds_duration(timeout));
    loop {
        if let Some(exit_code) = read_exit_code(&spool.exit_code) {
            let stdout = fs::read(&spool.stdout).unwrap_or_default();
            let stderr = fs::read(&spool.stderr).unwrap_or_default();
            let result = RunResult {
                exit_code,
                stdout,
                stderr,
                run_id,
                spool,
                target: target.clone(),
                readiness,
                duration_seconds: started.elapsed().as_secs_f64(),
            };
            FileExt::unlock(&lock).ok();
            return Ok(result);
        }
        if deadline.is_some_and(|value| Instant::now() >= value) {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    FileExt::unlock(&lock).ok();
    Err(HerdrRunError::timeout(format!(
        "command did not finish within {}s. It is STILL RUNNING in pane {} ({}) and was not killed. Partial output is in {}; the exit code will appear in {} when it finishes.",
        number_text(timeout),
        target.pane_id,
        target.tab_label,
        spool.directory.display(),
        spool.exit_code.display()
    )))
}

fn spool_error(spool: &SpoolPaths, error: std::io::Error) -> HerdrRunError {
    HerdrRunError::unavailable(format!(
        "cannot initialize run spool {}: {error}",
        spool.directory.display()
    ))
}

fn read_exit_code(path: &Path) -> Option<i32> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    let stripped = text.trim();
    (!stripped.is_empty())
        .then(|| stripped.parse::<i32>().ok())
        .flatten()
}

/// Write the completed run's structured evidence and return its path.
pub fn write_meta(
    result: &RunResult,
    admission: &Admission,
    config: &Config,
    agent: &str,
) -> Result<PathBuf> {
    let path = result.spool.directory.join("meta.json");
    let document = json!({
        "agent": agent,
        "argv": admission.argv,
        "config_source": config.source_path,
        "created": result.target.created,
        "duration_seconds": round_three(result.duration_seconds),
        "exit_code": result.exit_code,
        "from_cache": result.target.from_cache,
        "pane_id": result.target.pane_id,
        "prefix": admission.prefix,
        "program": admission.program,
        "readiness": {
            "foreground_pgid": result.readiness.process.foreground_pgid,
            "process_idle": result.readiness.process.idle,
            "process_reason": result.readiness.process.reason,
            "prompt_reason": result.readiness.prompt.reason,
            "prompt_tail": result.readiness.prompt.tail,
            "prompt_verdict": result.readiness.prompt.verdict,
            "shell_pid": result.readiness.process.shell_pid,
        },
        "rendered": admission.rendered(),
        "run_id": result.run_id,
        "subcommand": admission.subcommand,
        "tab": {"id": result.target.tab_id, "label": result.target.tab_label},
        "workspace": {"id": result.target.workspace_id, "label": result.target.workspace_label},
    });
    let mut encoded = serde_json::to_vec_pretty(&document).map_err(|error| {
        HerdrRunError::unavailable(format!("cannot encode run metadata: {error}"))
    })?;
    encoded.push(b'\n');
    create_private_file(&path, &encoded).map_err(|error| {
        HerdrRunError::unavailable(format!(
            "cannot write run metadata {}: {error}",
            path.display()
        ))
    })?;
    Ok(path)
}

/// Build the stable success JSON object used by the CLI.
#[must_use]
pub fn result_json(result: &RunResult, meta: Option<&Path>) -> Value {
    use base64::Engine as _;
    json!({
        "created": result.target.created,
        "duration_seconds": round_three(result.duration_seconds),
        "exit_code": result.exit_code,
        "meta": meta,
        "pane_id": result.target.pane_id,
        "run_id": result.run_id,
        "spool_dir": result.spool.directory,
        "stderr": result.stderr_text(),
        "stderr_base64": base64::engine::general_purpose::STANDARD.encode(&result.stderr),
        "stdout": result.stdout_text(),
        "stdout_base64": base64::engine::general_purpose::STANDARD.encode(&result.stdout),
        "tab": result.target.tab_label,
    })
}

fn round_three(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

fn seconds_duration(seconds: f64) -> Duration {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Duration::ZERO;
    }
    if seconds >= Duration::MAX.as_secs_f64() {
        return Duration::MAX;
    }
    Duration::from_secs_f64(seconds)
}

fn number_text(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Command;

    use super::*;
    use crate::allowlist::admit;
    use crate::client::{Pane, ProcessInfo};

    struct FakeApi;

    impl HerdrApi for FakeApi {
        fn ensure_server(&self) -> Result<bool> {
            Ok(false)
        }
        fn workspace_id_for_label(&self, _label: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn workspace_label_for_id(&self, _id: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn create_workspace(&self, _label: &str, _cwd: &str) -> Result<(String, String, String)> {
            unreachable!()
        }
        fn tab_id_for_label(&self, _workspace: &str, _label: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn create_tab(&self, _workspace: &str, _label: &str, _cwd: &str) -> Result<String> {
            unreachable!()
        }
        fn rename_tab(&self, _tab: &str, _label: &str) -> Result<()> {
            unreachable!()
        }
        fn panes(&self, _workspace: Option<&str>) -> Result<Vec<Pane>> {
            Ok(Vec::new())
        }
        fn pane_exists(&self, _pane: &str) -> bool {
            true
        }
        fn process_info(&self, pane: &str) -> Result<ProcessInfo> {
            Ok(ProcessInfo {
                pane_id: pane.to_owned(),
                shell_pid: 7,
                foreground_pgid: 7,
                foreground: vec![(7, "bash".to_owned(), "bash".to_owned())],
            })
        }
        fn read(&self, _pane: &str, _source: &str, _lines: Option<usize>) -> Result<String> {
            Ok(String::new())
        }
        fn run(&self, _pane: &str, command: &str) -> Result<()> {
            let status = Command::new("bash")
                .args(["-lc", command])
                .status()
                .map_err(|error| HerdrRunError::unavailable(error.to_string()))?;
            if status.success() {
                Ok(())
            } else {
                Err(HerdrRunError::unavailable(format!(
                    "fake pane failed: {status}"
                )))
            }
        }
        fn send_keys(&self, _pane: &str, _keys: &str) -> Result<()> {
            Ok(())
        }
    }

    fn target() -> Target {
        Target {
            workspace_id: "w1".to_owned(),
            tab_id: "t1".to_owned(),
            pane_id: "p1".to_owned(),
            workspace_label: "agent-cmds".to_owned(),
            tab_label: "agent".to_owned(),
            created: Vec::new(),
            from_cache: false,
        }
    }

    #[test]
    fn shell_wrapper_is_portable_outer_command_and_quotes_paths() {
        let config = Config {
            allow: vec!["git".to_owned()],
            ..Config::default()
        };
        let admission = admit("git status", &config).expect("admitted");
        let spool = spool_paths(
            &Config {
                project_root: "/tmp/root x".to_owned(),
                spool_dir: "sp ool".to_owned(),
                ..Config::default()
            },
            "run 1",
        );
        let line =
            build_shell_command(&admission, &spool, Path::new("/tmp/work dir")).expect("rendered");
        assert!(line.starts_with("command sh -c "));
        assert!(line.contains("'\"'\"'/tmp/work dir'\"'\"'"));
    }

    #[test]
    fn pane_lock_is_account_global_across_project_configs() {
        let root =
            std::env::temp_dir().join(format!("herdr-pane-lock-projects-{}", std::process::id()));
        let first_config = Config {
            project_root: root.join("one").to_string_lossy().into_owned(),
            spool_dir: "first-spool".to_owned(),
            ..Config::default()
        };
        let second_config = Config {
            project_root: root.join("two").to_string_lossy().into_owned(),
            spool_dir: "second-spool".to_owned(),
            ..Config::default()
        };
        assert_ne!(
            spool_paths(&first_config, "run").directory,
            spool_paths(&second_config, "run").directory
        );

        let mut target = target();
        target.pane_id = "pane-global-across-project-configs".to_owned();
        let first = acquire_pane_lock(&target, 0.0).expect("first account-global lock");
        let second = acquire_pane_lock(&target, 0.0).unwrap_err();
        assert_eq!(second.kind(), crate::error::ErrorKind::Busy);
        drop(first);
    }

    #[test]
    fn execution_captures_bytes_exit_and_uses_exclusive_collision_suffix() {
        let root = std::env::temp_dir().join(format!("herdr-runner-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let config = Config {
            project_root: root.to_string_lossy().into_owned(),
            spool_dir: "spool".to_owned(),
            allow: vec!["printf".to_owned()],
            ..Config::default()
        };
        let admission = admit("printf '\\377'", &config).expect("admitted");
        let result = execute(
            &FakeApi,
            &config,
            &target(),
            &admission,
            "agent",
            &root,
            0.0,
            5.0,
        )
        .expect("run");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, vec![0xff]);
        let meta = write_meta(&result, &admission, &config, "agent").expect("metadata");
        assert!(result_json(&result, None)["meta"].is_null());
        for directory in [
            root.join("spool"),
            root.join("spool/runs"),
            result.spool.directory.clone(),
        ] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for file in [
            result.spool.stdout.clone(),
            result.spool.stderr.clone(),
            result.spool.exit_code.clone(),
            result.spool.directory.join("command"),
            meta,
        ] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let base = make_run_id("agent", SystemTime::now(), std::process::id());
        let first = spool_paths(&config, &base);
        fs::create_dir_all(&first.directory).expect("plant collision");
        fs::write(&first.exit_code, "99\n").expect("plant stale status");
        let (second_id, second) = allocate_spool(&config, &base).expect("new spool");
        assert_ne!(second_id, base);
        assert!(!second.exit_code.exists());
        let _ = fs::remove_dir_all(root);
    }
}
