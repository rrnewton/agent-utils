//! Slack provider for the agentctl chat bridge.
//!
//! The bridge drives a provider through two separate executables, and this crate supplies both:
//!
//! * `agentctl-slack-plugin` speaks the `agentctl-chat-subscription` v1 process protocol. It
//!   turns `message.created` events from one Slack channel into ordered inbound requests
//!   ([`plugin`]).
//! * `agentctl-slack-outbound` is the bridge's one-shot `outbound_command`. Each run reads one
//!   JSON request line, posts a reply into the request's Slack thread or adds the
//!   acknowledgement reaction, and writes one JSON response line ([`outbound`]). Run with
//!   `--role=warm-worker`, the same executable is a long-lived worker that keeps a Slack client
//!   started for a likely reply, so the helper's post skips the client's start-up ([`warm`]).
//!
//! Both talk to Slack through the small [`transport::SlackTransport`] interface. The
//! [`cli::CliTransport`] implementation runs an installed Slack command-line client whose path
//! the operator configures; [`transport::WebApiTransport`] is a placeholder that refuses every
//! operation, and [`fake::FakeTransport`] is the deterministic transport the tests use.
//!
//! Outbound posts always act as a registered Slack app (bot identity). The transport interface
//! takes an [`ids::AppId`] for every write, so a post or reaction cannot be built without one,
//! and the outbound helper refuses every request when no app id is configured.

#![deny(unsafe_code)]

pub mod cli;
pub mod diag;
pub mod fake;
pub mod ids;
pub mod outbound;
pub mod plugin;
pub mod refusals;
pub mod transport;
pub mod warm;

/// Release version reported in diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
