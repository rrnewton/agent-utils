//! Headless agents: a harness driven over its structured protocol instead of a terminal
//! (`#565 headless-coordinator`; design in
//! `ai_docs/transient/2026-10-26-headless-coordinator/README.md`).
//!
//! - [`events`]: the typed JSONL event log a headless agent writes (M1).
//! - [`tail`]: `agentctl tail NAME`, a read-only follower of that log (M1).
//! - [`control`]: the wrapper's control socket (wake, status, interrupt, stop).
//! - [`claude`]: Claude Code's stream-json protocol, translated.
//! - [`wrapper`]: the Claude Code wrapper, the queue's only drainer (M2).
//!
//! A headless agent's record has `adapter: harness-wrapper` and `mode: headless`. Its files sit
//! in its registry directory beside the record:
//!
//! ```text
//! <registry>/<agent>/agent.json      the record
//! <registry>/<agent>/queue/          the durable queue (inbox.v2 lane); only the wrapper drains it
//! <registry>/<agent>/events/         the event log
//! <registry>/<agent>/control.sock    the wrapper's control socket, while it runs
//! <registry>/<agent>/headless.lock   held by the running wrapper
//! ```

pub mod claude;
pub mod control;
pub mod events;
pub mod tail;
pub mod wrapper;

/// The `adapter` value of a headless agent's record.
pub const ADAPTER: &str = "harness-wrapper";
/// The `mode` value of a headless agent's record.
pub const MODE: &str = "headless";
/// Harnesses this build can run headless.
pub const HARNESSES: &[&str] = &["claude"];

/// Example settings for a headless coordinator, kept beside this module.
pub const EXAMPLE_SETTINGS: &str = include_str!("coordinator-settings.example.json");

/// What `agentctl capabilities` reports about headless support in this build.
#[must_use]
pub fn capabilities() -> serde_json::Value {
    serde_json::json!({
        "adapter": ADAPTER,
        "harnesses": HARNESSES,
        "event_log": format!("v{}", events::VERSION),
        "commands": ["start --mode headless", "run-headless", "tail", "interrupt", "compact",
                     "send", "drain", "status", "stop", "install-units --headless"],
        "claude": claude::capabilities(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn example_settings_parse_and_carry_deny_rules() {
        let settings: serde_json::Value = serde_json::from_str(super::EXAMPLE_SETTINGS).unwrap();
        let deny = settings["permissions"]["deny"].as_array().unwrap();
        assert!(deny.iter().any(|rule| rule == "Bash(git push --force*)"));
        assert!(!settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
