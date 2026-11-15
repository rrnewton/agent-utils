//! The headless harness-adapter seam (`#1232 headless-harness-adapters`).
//!
//! The wrapper owns the durable queue, the typed event log, approvals and
//! recovery; everything that differs between harnesses lives behind this
//! trait: launch arguments, the wire form of one queued input, translation
//! of harness output into [`Signal`]s, control lines, and the transcript
//! that proves an input was received. [`super::claude`] is the first
//! implementation; a second harness implements this trait without further
//! wrapper surgery.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::claude::{self, Signal};

/// One headless harness the wrapper can drive.
pub trait HarnessAdapter: std::fmt::Debug + Send + Sync {
    /// Harness name as records and `agentctl capabilities` spell it.
    fn name(&self) -> &'static str;

    /// What `agentctl capabilities` reports for this harness.
    fn capabilities(&self) -> Value;

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

    /// A fresh caller-chosen input identifier.
    fn new_input_id(&self) -> std::io::Result<String>;

    /// The wire form of one queued input.
    fn input_line(&self, uuid: &str, text: &str, session: &str) -> String;

    /// What one parsed harness output value means to the wrapper.
    fn parse(&self, line: &Value) -> Vec<Signal>;

    /// The wire form that interrupts the running turn.
    fn interrupt_line(&self, request_id: &str) -> String;

    /// The wire form that refuses a control request the wrapper does not serve.
    fn control_error_line(&self, request_id: &str, error: &str) -> String;

    /// The wire form that answers permission prompt `request_id`.
    fn permission_answer_line(
        &self,
        request_id: &str,
        allow: bool,
        input: &Value,
        message: &str,
    ) -> String;

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

/// Every adapter this build can run headless, in report order.
#[must_use]
pub fn adapters() -> Vec<&'static dyn HarnessAdapter> {
    vec![&claude::CLAUDE_HARNESS]
}

/// The adapter for harness `name`, when this build has one.
#[must_use]
pub fn adapter_for(name: &str) -> Option<&'static dyn HarnessAdapter> {
    adapters()
        .into_iter()
        .find(|adapter| adapter.name() == name)
}

/// Names of the harnesses this build can run headless.
#[must_use]
pub fn harness_names() -> Vec<&'static str> {
    adapters().iter().map(|adapter| adapter.name()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_names_match_the_adapters() {
        assert_eq!(harness_names(), ["claude"]);
        assert!(adapter_for("claude").is_some());
        assert!(adapter_for("muse").is_none());
    }
}
