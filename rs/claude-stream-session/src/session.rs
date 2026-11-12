//! One persistent `claude -p --input-format stream-json` process, driven turn by turn.
//!
//! The process stays warm across [`Session::complete`] calls, so a caller pays process
//! startup once instead of per turn. A turn that finds the process dead fails by name and
//! the next call respawns; recycling policy (turns, tokens) belongs to the caller's pool,
//! which reads [`Session::turns`] and [`Session::tokens`] and drops the session.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

use crate::protocol;

/// How to start one session process.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// The `claude` program (a path, for tests and non-default installs).
    pub program: String,
    /// Model passed as `--model`, when set.
    pub model: Option<String>,
    /// Effort passed as `--effort`, when set.
    pub effort: Option<String>,
    /// Extra operator arguments, validated against [`protocol::RESERVED_ARGUMENTS`].
    pub arguments: Vec<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            program: "claude".to_owned(),
            model: None,
            effort: None,
            arguments: Vec::new(),
        }
    }
}

/// One completed turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    /// The assistant text of the turn, blocks joined in order.
    pub text: String,
    /// Total tokens the turn reported (input + output), when it reported usage.
    pub tokens: Option<u64>,
}

/// Why a turn could not complete.
#[derive(Debug)]
pub enum Error {
    /// The session process could not be started or written to.
    Spawn(std::io::Error),
    /// The session process ended before the turn's `result` line.
    Died,
    /// The harness answered with an error result.
    Refused(String),
    /// The harness output could not be understood.
    Shape(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "claude session could not start: {error}"),
            Self::Died => write!(formatter, "claude session ended mid-turn"),
            Self::Refused(message) => write!(formatter, "claude session refused: {message}"),
            Self::Shape(message) => {
                write!(formatter, "claude session answered unusably: {message}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// A warm session process. Not `Sync`: one turn at a time, in call order.
pub struct Session {
    config: SessionConfig,
    session_id: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    turns: u64,
    tokens: u64,
}

impl Session {
    /// Start the process now, so the first turn does not pay startup.
    ///
    /// # Errors
    ///
    /// [`Error::Spawn`] when the program cannot be started, or a plain message when the
    /// configured arguments are reserved.
    pub fn spawn(config: SessionConfig) -> Result<Self, Error> {
        protocol::validate_arguments(&config.arguments).map_err(Error::Shape)?;
        let session_id = protocol::random_uuid().map_err(Error::Spawn)?;
        let mut session = Self {
            config,
            session_id,
            child: None,
            stdin: None,
            stdout: None,
            turns: 0,
            tokens: 0,
        };
        session.start()?;
        Ok(session)
    }

    fn start(&mut self) -> Result<(), Error> {
        let mut argv = protocol::protocol_arguments();
        argv.extend(protocol::effective_arguments(&self.config.arguments, false));
        // A summariser session never runs tools: an empty tool list, always.
        argv.push("--tools".to_owned());
        argv.push(String::new());
        if let Some(model) = &self.config.model {
            argv.push("--model".to_owned());
            argv.push(model.clone());
        }
        if let Some(effort) = &self.config.effort {
            argv.push("--effort".to_owned());
            argv.push(effort.clone());
        }
        let mut child = Command::new(&self.config.program)
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(Error::Spawn)?;
        self.stdin = child.stdin.take();
        self.stdout = child.stdout.take().map(BufReader::new);
        self.child = Some(child);
        if self.stdin.is_none() || self.stdout.is_none() {
            return Err(Error::Died);
        }
        Ok(())
    }

    /// Whether the process is still running.
    pub fn alive(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Turns completed on this process.
    #[must_use]
    pub fn turns(&self) -> u64 {
        self.turns
    }

    /// Tokens reported across this process's turns.
    #[must_use]
    pub fn tokens(&self) -> u64 {
        self.tokens
    }

    /// Run one turn and return its text, respawning first if the process died.
    ///
    /// # Errors
    ///
    /// [`Error`] when the process cannot run or the turn fails; a dead process fails the
    /// in-flight turn as [`Error::Died`] rather than silently replaying it.
    pub fn complete(&mut self, prompt: &str) -> Result<Turn, Error> {
        if !self.alive() {
            self.start()?;
        }
        let uuid = protocol::random_uuid().map_err(Error::Spawn)?;
        let line = protocol::input_line(&uuid, prompt, &self.session_id);
        let stdin = self.stdin.as_mut().ok_or(Error::Died)?;
        stdin.write_all(line.as_bytes()).map_err(|_| Error::Died)?;
        stdin.flush().map_err(|_| Error::Died)?;

        let mut text = String::new();
        let mut tokens = None;
        let stdout = self.stdout.as_mut().ok_or(Error::Died)?;
        let mut raw = String::new();
        loop {
            raw.clear();
            let read = stdout.read_line(&mut raw).map_err(|_| Error::Died)?;
            if read == 0 {
                return Err(Error::Died);
            }
            let Ok(value) = serde_json::from_str::<Value>(raw.trim()) else {
                continue;
            };
            match value.get("type").and_then(Value::as_str) {
                Some("assistant") => {
                    if let Some(blocks) =
                        value.pointer("/message/content").and_then(Value::as_array)
                    {
                        for block in blocks {
                            if block.get("type").and_then(Value::as_str) == Some("text") {
                                if let Some(part) = block.get("text").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        text.push('\n');
                                    }
                                    text.push_str(part);
                                }
                            }
                        }
                    }
                }
                Some("result") => {
                    if let Some(usage) = value.get("usage") {
                        let sum = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
                        tokens = Some(sum("input_tokens") + sum("output_tokens"));
                    }
                    if value.get("subtype").and_then(Value::as_str) == Some("success") {
                        if text.is_empty() {
                            if let Some(result) = value.get("result").and_then(Value::as_str) {
                                text = result.to_owned();
                            }
                        }
                        self.turns += 1;
                        if let Some(tokens) = tokens {
                            self.tokens += tokens;
                        }
                        return Ok(Turn { text, tokens });
                    }
                    let message = value
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("error result")
                        .to_owned();
                    return Err(Error::Refused(message));
                }
                _ => {}
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
