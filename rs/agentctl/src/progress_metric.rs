//! A progress metric declared for an agent (`agentctl progress`, #570 agent-progress-monitoring).
//!
//! Activity is not progress. A lane can write a status line every hour, keep its screen moving,
//! and still not move its goal for a day; one did, and its own status file said so ("remain
//! ZERO") while every outside signal said it was busy. A dispatcher that knows what progress
//! looks like declares it: a file the lane updates, optionally narrowed to the lines that carry
//! the count, or a command whose output is the count. `agentctl watch-agents` reads it every
//! pass and tells the coordinator when it has not changed for the declared number of hours while
//! the agent is working.
//!
//! Declarations live in `REGISTRY/.progress/NAME.json` (schema `agentctl-progress/v1`), beside
//! the records and outside every agent's own directory.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::agent::AgentError;

/// Schema of a declaration.
pub const SCHEMA: &str = "agentctl-progress/v1";
/// Bytes of a metric file read.
const FILE_LIMIT: u64 = 1 << 20;
/// Bytes of a metric command's output kept.
const OUTPUT_LIMIT: u64 = 64 << 10;
/// How long a metric command may run.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Characters of a metric value quoted in a notice.
const QUOTED_CHARS: usize = 120;

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// What counts as progress for one agent.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Declaration {
    /// Always [`SCHEMA`].
    pub schema: String,
    /// A file the agent updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Only the file's lines containing this text count (default: the whole file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matching: Option<String>,
    /// A shell command whose standard output is the metric, run in the agent's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Hours the metric may stay unchanged while the agent works before the coordinator is told.
    pub hours: f64,
    /// Unix seconds the declaration was written.
    pub declared_at: f64,
}

/// One reading of a metric.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The metric's current value, when it could be read.
    pub value: Option<String>,
    /// Why it could not be read.
    pub error: Option<String>,
    /// How long it may stay unchanged, in seconds.
    pub window_seconds: f64,
    /// What was read, for a notice: the file or the command.
    pub source: String,
}

impl Reading {
    /// The value bounded for a notice: its last line, which for an appended status file is the
    /// newest.
    #[must_use]
    pub fn quoted(&self) -> String {
        self.value
            .as_deref()
            .and_then(|value| value.lines().rev().find(|line| !line.trim().is_empty()))
            .map(|line| line.trim().chars().take(QUOTED_CHARS).collect())
            .unwrap_or_default()
    }
}

/// Where `name`'s declaration lives in `registry`.
#[must_use]
pub fn path(registry: &Path, name: &str) -> PathBuf {
    registry.join(".progress").join(format!("{name}.json"))
}

/// `name`'s declaration, or `None` when it has none.
pub fn load(registry: &Path, name: &str) -> Result<Option<Declaration>> {
    let path = path(registry, name);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail(format!("cannot read {}: {error}", path.display()))),
    };
    let declaration: Declaration = serde_json::from_str(&text).map_err(|error| {
        fail(format!(
            "{} is not a progress declaration: {error}",
            path.display()
        ))
    })?;
    if declaration.schema != SCHEMA {
        return Err(fail(format!(
            "{} has schema {:?}, expected {SCHEMA}",
            path.display(),
            declaration.schema
        )));
    }
    Ok(Some(declaration))
}

/// Write `name`'s declaration: a file or a command, and the hours it may stay unchanged.
pub fn declare(
    registry: &Path,
    name: &str,
    file: Option<PathBuf>,
    matching: Option<String>,
    command: Option<String>,
    hours: f64,
) -> Result<Declaration> {
    if file.is_some() == command.is_some() {
        return Err(fail("declare exactly one of a file or a command"));
    }
    if matching.is_some() && file.is_none() {
        return Err(fail("--matching narrows a --file metric"));
    }
    if !(hours.is_finite() && hours > 0.0) {
        return Err(fail("hours must be a positive number"));
    }
    let declaration = Declaration {
        schema: SCHEMA.to_owned(),
        file,
        matching,
        command,
        hours,
        declared_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64()),
    };
    let path = path(registry, name);
    let directory = path.parent().expect("declaration path has a directory");
    crate::agent::create_private_directory(directory, "progress declarations", false, false)?;
    let value = serde_json::to_value(&declaration).map_err(|error| fail(error.to_string()))?;
    crate::agent::atomic_json(&path, &value)?;
    Ok(declaration)
}

/// Remove `name`'s declaration; `true` when there was one.
pub fn clear(registry: &Path, name: &str) -> Result<bool> {
    let path = path(registry, name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fail(format!("cannot remove {}: {error}", path.display()))),
    }
}

/// Read the metric `declaration` describes; a command runs in `cwd`.
#[must_use]
pub fn read(declaration: &Declaration, cwd: &Path) -> Reading {
    let mut reading = Reading {
        window_seconds: declaration.hours * 3600.0,
        ..Reading::default()
    };
    if let Some(file) = &declaration.file {
        let file = if file.is_absolute() {
            file.clone()
        } else {
            cwd.join(file)
        };
        reading.source = file.display().to_string();
        let mut text = String::new();
        match fs::File::open(&file)
            .and_then(|handle| handle.take(FILE_LIMIT).read_to_string(&mut text))
        {
            Ok(_) => {
                reading.value = Some(match &declaration.matching {
                    Some(needle) => text
                        .lines()
                        .filter(|line| line.contains(needle.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    None => text,
                });
            }
            Err(error) => reading.error = Some(format!("cannot read {}: {error}", file.display())),
        }
    } else if let Some(command) = &declaration.command {
        reading.source = format!("`{command}`");
        match run(command, cwd) {
            Ok(output) => reading.value = Some(output),
            Err(error) => reading.error = Some(error),
        }
    }
    reading
}

/// Run `command` with `sh -c` in `cwd`, bounded in time and output; its standard output, or why
/// it failed.
fn run(command: &str, cwd: &Path) -> std::result::Result<String, String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run the metric command: {error}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(OUTPUT_LIMIT).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the metric command ran past {} s and was stopped",
                    COMMAND_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("cannot wait for the metric command: {error}")),
        }
    };
    let bytes = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("the metric command failed ({status})"));
    }
    Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        crate::turn_state::tests::scratch()
    }

    #[test]
    fn a_declaration_round_trips_and_clears() {
        let registry = scratch();
        assert_eq!(load(&registry, "lane").unwrap(), None);
        let declared = declare(
            &registry,
            "lane",
            Some(PathBuf::from("status.md")),
            Some("crashes recorded".to_owned()),
            None,
            6.0,
        )
        .unwrap();
        assert_eq!(load(&registry, "lane").unwrap(), Some(declared));
        assert!(clear(&registry, "lane").unwrap());
        assert!(!clear(&registry, "lane").unwrap());
        assert!(declare(&registry, "lane", None, None, None, 1.0).is_err());
        assert!(declare(
            &registry,
            "lane",
            None,
            Some("x".into()),
            Some("true".into()),
            1.0
        )
        .is_err());
        assert!(declare(&registry, "lane", None, None, Some("true".into()), 0.0).is_err());
        fs::remove_dir_all(&registry).unwrap();
    }

    #[test]
    fn a_file_metric_counts_only_its_matching_lines() {
        let directory = scratch();
        fs::write(
            directory.join("status.md"),
            "09:00 heartbeat\ncrashes recorded: 0\n10:00 heartbeat\n",
        )
        .unwrap();
        let declaration = declare(
            &directory,
            "lane",
            Some(PathBuf::from("status.md")),
            Some("crashes recorded".to_owned()),
            None,
            6.0,
        )
        .unwrap();
        let first = read(&declaration, &directory);
        // An hourly heartbeat is activity, not progress: the metric does not move.
        fs::write(
            directory.join("status.md"),
            "09:00 heartbeat\ncrashes recorded: 0\n10:00 heartbeat\n11:00 heartbeat\n",
        )
        .unwrap();
        let second = read(&declaration, &directory);
        assert_eq!(first.value, second.value);
        assert_eq!(second.quoted(), "crashes recorded: 0");
        assert_eq!(second.window_seconds, 6.0 * 3600.0);
        fs::write(directory.join("status.md"), "crashes recorded: 1\n").unwrap();
        assert_ne!(read(&declaration, &directory).value, second.value);
        fs::remove_file(directory.join("status.md")).unwrap();
        assert!(read(&declaration, &directory).error.is_some());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_command_metric_is_its_output_and_a_failure_is_an_error() {
        let directory = scratch();
        let ok = Declaration {
            schema: SCHEMA.to_owned(),
            file: None,
            matching: None,
            command: Some("printf '3 replayed\\n'".to_owned()),
            hours: 1.0,
            declared_at: 0.0,
        };
        let reading = read(&ok, &directory);
        assert_eq!(reading.value.as_deref(), Some("3 replayed"));
        assert_eq!(reading.source, "`printf '3 replayed\\n'`");
        let failing = Declaration {
            command: Some("exit 3".to_owned()),
            ..ok
        };
        let reading = read(&failing, &directory);
        assert_eq!(reading.value, None);
        assert!(reading.error.unwrap().contains("failed"));
        fs::remove_dir_all(&directory).unwrap();
    }
}
