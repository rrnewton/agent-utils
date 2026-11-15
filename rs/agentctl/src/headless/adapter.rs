//! The headless harness-adapter seam (`#1232 headless-harness-adapters`).
//!
//! The wrapper owns the durable queue, the typed event log, approvals and
//! recovery; everything that differs between harnesses lives behind this
//! trait: launch arguments, the wire form of one queued input, translation
//! of harness output into [`Signal`]s, control lines, and the transcript
//! that proves an input was received. [`super::claude`] is the first
//! implementation; [`super::muse`] is the second.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use super::claude::{self, Signal};
use super::muse;

/// One headless harness the wrapper can drive. Adapters may keep
/// per-launch state (a protocol's server version, a session it confirmed),
/// so the wrapper holds one instance per agent, not a shared static.
pub trait HarnessAdapter: std::fmt::Debug + Send + Sync {
    /// Harness name as records and `agentctl capabilities` spell it.
    fn name(&self) -> &'static str;

    /// What `agentctl capabilities` reports for this harness.
    fn capabilities(&self) -> Value;

    /// The harness executable the wrapper runs when none is named.
    fn default_binary(&self) -> &'static str;

    /// Arguments after the harness command for one launch: the protocol
    /// arguments, the session argument (`resume` when a transcript already
    /// proves the session), and the operator arguments as this harness
    /// takes them.
    fn launch_arguments(
        &self,
        resume: bool,
        session: &str,
        arguments: &[String],
        approvals: bool,
    ) -> Vec<String>;

    /// Lines the wrapper writes as soon as the harness starts, before any
    /// queued input (a protocol handshake, a session open). Most harnesses
    /// need none.
    fn hello_lines(
        &self,
        resume: bool,
        session: &str,
        cwd: &Path,
        effort: Option<&str>,
        approvals: bool,
    ) -> Vec<String> {
        let _ = (resume, session, cwd, effort, approvals);
        Vec::new()
    }

    /// Whether queued inputs must wait for the harness's first
    /// [`Signal::Init`] (a handshake has to open the session first).
    fn needs_init_before_input(&self) -> bool {
        false
    }

    /// Whether an [`Signal::Init`] opens a turn. Claude Code emits
    /// `system/init` at the start of every turn; a session host that
    /// reports its session once at open (Muse) does not.
    fn init_begins_turn(&self) -> bool {
        true
    }

    /// A fresh caller-chosen input identifier.
    fn new_input_id(&self) -> std::io::Result<String>;

    /// The wire form of one queued input. `effort` is the launch's declared
    /// reasoning effort, for harnesses that take it per input.
    fn input_line(&self, uuid: &str, text: &str, session: &str, effort: Option<&str>) -> String;

    /// What one parsed harness output value means to the wrapper.
    fn parse(&self, line: &Value) -> Vec<Signal>;

    /// The wire form that interrupts the running turn. `turn` is the
    /// in-flight input's identifier, when one is known.
    fn interrupt_line(&self, request_id: &str, session: &str, turn: Option<&str>) -> String;

    /// The wire form that refuses a control request the wrapper does not serve.
    fn control_error_line(&self, request_id: &str, error: &str) -> String;

    /// The wire form that answers permission prompt `request_id`.
    /// `requirement` and `choices` carry the prompt's own decision stage
    /// and offered choices for harnesses whose answer names them.
    #[allow(clippy::too_many_arguments)]
    fn permission_answer_line(
        &self,
        request_id: &str,
        allow: bool,
        input: &Value,
        message: &str,
        requirement: Option<&Value>,
        choices: &[(String, String, String)],
        session: &str,
    ) -> String;

    /// A receipt written as soon as a permission prompt is presented,
    /// for harnesses whose wire separates presenting a prompt from
    /// deciding it (Muse's MSP presentation receipt).
    fn permission_receipt_line(
        &self,
        request: &super::claude::PermissionRequest,
    ) -> Option<String> {
        let _ = request;
        None
    }

    /// Whether this launch asks the wrapper (rather than nobody, or the
    /// harness itself) to answer permission prompts.
    fn asks_wrapper(&self, arguments: &[String], approvals: bool) -> bool;

    /// Replace values that look like credentials in `text`.
    fn mask_secrets(&self, text: &str) -> String;

    /// The refusal text the model reads for a denied call.
    fn denial_text(&self, text: &str) -> String;

    /// The harness configuration directory, for transcript lookups.
    fn config_directory(&self) -> Option<PathBuf>;

    /// The transcript of `session` running in `cwd`, when the harness keeps one.
    fn transcript_path(&self, cwd: &Path, session: &str, config: &Path) -> PathBuf;

    /// Whether the transcript proves the model received input `uuid`.
    fn transcript_received(&self, transcript: &Path, uuid: &str) -> bool;

    /// Lines of each tool result kept as its preview by default.
    fn preview_lines(&self) -> usize;

    /// Whether `session` is a session identifier this harness accepts.
    fn valid_session_id(&self, session: &str) -> bool;

    /// Refuse operator arguments this harness cannot run headless.
    fn validate_arguments(&self, arguments: &[String]) -> Result<(), String>;
}

/// Every adapter this build can run headless, in report order. Each call
/// builds fresh instances: an adapter's per-launch state belongs to one
/// agent's wrapper.
#[must_use]
pub fn adapters() -> Vec<Arc<dyn HarnessAdapter>> {
    vec![
        Arc::new(claude::ClaudeHarness),
        Arc::new(muse::MuseHarness::serve()),
    ]
}

/// The adapter for harness `name`, when this build has one.
#[must_use]
pub fn adapter_for(name: &str) -> Option<Arc<dyn HarnessAdapter>> {
    adapters()
        .into_iter()
        .find(|adapter| adapter.name() == name)
}

/// Names of the harnesses this build can run headless.
#[must_use]
pub fn harness_names() -> Vec<&'static str> {
    adapters().iter().map(|adapter| adapter.name()).collect()
}

/// The declared reasoning effort in launch `arguments` (`--effort LEVEL`
/// or `--effort=LEVEL`), as `start` and `run-headless` record it (#1216).
#[must_use]
pub fn declared_effort(arguments: &[String]) -> Option<&str> {
    let mut iter = arguments.iter();
    while let Some(argument) = iter.next() {
        if argument == "--effort" {
            return iter.next().map(String::as_str);
        }
        if let Some(level) = argument.strip_prefix("--effort=") {
            return Some(level);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_names_match_the_adapters() {
        assert_eq!(harness_names(), ["claude", "muse"]);
        assert!(adapter_for("claude").is_some());
        assert!(adapter_for("muse").is_some());
        assert!(adapter_for("codex").is_none());
    }

    #[test]
    fn declared_effort_reads_both_spellings() {
        assert_eq!(
            declared_effort(&["--effort".into(), "high".into()]),
            Some("high")
        );
        assert_eq!(declared_effort(&["--effort=max".into()]), Some("max"));
        assert_eq!(declared_effort(&["--model".into(), "m".into()]), None);
    }
}
