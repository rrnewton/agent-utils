//! Discord provider for the agentctl chat bridge and the agent-teams chat relay.
//!
//! Two machines' agents talk to each other through one shared Discord channel. Each host runs
//! its own bot account; this crate is the provider that host's bridge uses:
//!
//! * `agentctl-discord-plugin` speaks the `agentctl-chat-subscription` v1 process protocol. It
//!   polls the configured channels over the Discord REST API ([`plugin`]), applies the
//!   addressing rule ([`addressing`]) and hands the bridge ordered, durably cursored events.
//!   Owner posts addressed to this host become ordinary requests (delivered through the
//!   bridge's existing durable queue); peer-bot posts are relay envelopes for
//!   `agent-teams relay-in`, never prompts.
//! * `agentctl-discord-outbound` is the bridge's one-shot `outbound_command`: it posts replies
//!   (as Discord replies to the request's root message) and acknowledgement reactions
//!   ([`outbound`]).
//!
//! Both talk to Discord through [`transport::DiscordRest`]. [`transport::CurlTransport`] runs
//! the system `curl` with its configuration (including the `Authorization` header) on standard
//! input, so the bot token never appears in a process argument list or a log line;
//! [`transport::FakeRest`] is the deterministic transport the tests use.
//!
//! Relay traffic is not reinvented here: peer-bot posts carry `agent-teams`' signed fenced
//! envelope and go, unchanged, to the bridge's `relay_channels` command (`agent-teams
//! relay-in`), whose ledger provides exactly-once delivery. [`admission`] is only the small
//! provider payload recording how this host admitted a post; [`markdown`] neutralises mentions
//! and markdown in visible text (the Discord counterpart of the relay's Slack link-markup
//! handling).

#![deny(unsafe_code)]

pub mod addressing;
pub mod admission;
pub mod ids;
pub mod markdown;
pub mod outbound;
pub mod plugin;
pub mod transport;

/// Release version reported in diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
