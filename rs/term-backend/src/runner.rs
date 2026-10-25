//! The external-command seam both CLI backends run through, so their
//! argument shapes are testable without a live server.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::{Result, TermError};

/// Captured result of one external command invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    /// Conventional numeric process status; zero denotes success.
    pub status: i32,
    /// Captured standard output decoded as UTF-8 with replacement.
    pub stdout: String,
    /// Captured standard error decoded as UTF-8 with replacement.
    pub stderr: String,
}

/// Runs one external command with optional stdin and captures its output.
pub trait CommandRunner: Send + Sync {
    /// Run `program` with `args`, feeding `stdin` when given.
    fn run(&self, program: &Path, args: &[String], stdin: Option<&str>) -> Result<CommandOutput>;
}

/// The production [`CommandRunner`]: a direct child process.
#[derive(Clone, Debug, Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &Path, args: &[String], stdin: Option<&str>) -> Result<CommandOutput> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = command.spawn().map_err(|error| {
            TermError::unavailable(format!("cannot run {}: {error}", program.display()))
        })?;
        if let Some(input) = stdin {
            let mut pipe = child.stdin.take().expect("piped stdin");
            pipe.write_all(input.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A runner shared into a backend; the production default is [`SystemRunner`].
pub(crate) type SharedRunner = Arc<dyn CommandRunner>;

/// The default production runner.
pub(crate) fn system_runner() -> SharedRunner {
    Arc::new(SystemRunner)
}

/// Join a failed command's stderr (or stdout) into one bounded detail line.
pub(crate) fn detail(output: &CommandOutput) -> String {
    let raw = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        format!("exit status {}", output.status)
    } else {
        trimmed.chars().take(400).collect()
    }
}

/// Require a zero exit, naming `purpose` in the refusal.
pub(crate) fn expect_ok(output: &CommandOutput, purpose: &str) -> Result<()> {
    if output.status == 0 {
        Ok(())
    } else {
        Err(TermError::failed(format!("{purpose}: {}", detail(output))))
    }
}
