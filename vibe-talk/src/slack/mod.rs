//! Slack implementation of the provider-neutral chat interface.
//!
//! [`HttpSlackClient`] reads and posts through a fixed subset of the Slack Web API, so a
//! deployment can point it at Slack itself or at a compatible bridge that implements exactly that
//! subset:
//!
//! | Method | Used for |
//! |---|---|
//! | `auth.test` | [`crate::chat::ChatClient::identity`], and the account this client posts as |
//! | `conversations.info` | verifying a conversation before it is registered |
//! | `conversations.history` | a conversation's main timeline, the thread list, reply targets |
//! | `conversations.replies` | a thread's messages, thread membership checks, exact lookups |
//! | `chat.postMessage` | every post, in a conversation or in a thread |
//! | `users.info` | an author's display name when the message does not carry one |
//! | `users.conversations` | the channel browser |
//!
//! Every call is a form-encoded `POST {api_base}/{method}` with `Authorization: Bearer <token>`.
//!
//! # Identity mapping
//!
//! The rest of the server is snowflake-ordered, so a Slack `ts` is encoded as a snowflake-shaped
//! [`crate::model::MessageId`] that orders, pages and dates exactly as the `ts` does; see
//! [`ids`]. A whole conversation's channel id is its Slack id, and a channel narrowed to one
//! thread is `"{conversation}~{thread_ts}"`. Thread ids in the thread APIs are root `ts` values.
//!
//! # What it does not do
//!
//! There is no Events API or Socket Mode connection: new messages arrive through the server's
//! ordinary bounded live polling, which reads a conversation's main timeline (thread replies are
//! read when a thread or a thread-scoped channel is opened). There are no upstream read marks.

mod client;
mod history;
pub mod ids;
pub mod mrkdwn;
mod timeline;

pub use client::HttpSlackClient;
pub use ids::{message_id_from_ts, parse_source, ts_from_message_id};

/// The Slack Web API base used when a deployment does not name a bridge.
pub const DEFAULT_SLACK_API_BASE: &str = "https://slack.com/api";

/// Slack access parameters for one workspace connection.
#[derive(Debug, Clone)]
pub struct SlackConfig {
    /// Display name of the chat service, normally `Slack`.
    pub provider_name: String,
    /// Web API base, such as [`DEFAULT_SLACK_API_BASE`]. A trailing slash is tolerated.
    pub api_base: String,
    /// Sent as `Authorization: Bearer <token>`: an `xoxb-` bot token, an `xoxp-` user token, or
    /// the token of a compatible bridge.
    pub token: crate::config::Secret,
    /// The owner's own Slack user id (`U…` or `W…`), when configured.
    pub owner_user_id: Option<String>,
    /// Seconds one HTTP request may take.
    pub request_timeout_seconds: u64,
    /// Whether the app's "Add channel" accepts Slack links and conversation ids as sources.
    pub channel_registration: bool,
    /// The `writable` policy reported for channels registered through the app.
    pub registered_channels_writable: bool,
    /// Whether the channel browser lists the conversations this account belongs to.
    pub channel_discovery: bool,
}
