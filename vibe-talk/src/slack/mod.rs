//! Slack implementation of the provider-neutral chat interface.
//!
//! [`HttpSlackClient`] reads and posts through a fixed subset of the Slack Web API, so a
//! deployment can point it at Slack itself or at a compatible bridge that implements exactly that
//! subset:
//!
//! | Method | Used for |
//! |---|---|
//! | `auth.test` | [`crate::chat::ChatClient::identity`], and the account this client posts as |
//! | `conversations.info` | verifying a conversation before it is registered, and naming a channel a message mentions without its name |
//! | `conversations.history` | a conversation's main timeline, the thread list, reply targets |
//! | `conversations.replies` | a thread's messages, thread membership checks, exact lookups |
//!
//! A lookup of several messages by id reads `conversations.history` or `conversations.replies`
//! a run of timestamps a call (`slack/lookup.rs`). A read of several threads reads each thread's
//! newest page in one `conversations.replies` window, and after an instant only the threads whose
//! root moved (`slack/threads.rs`). The live poller reads the newest page of history, and the new
//! replies of only the threads whose root moved (`slack/live.rs`); a read of the newest messages
//! adds the replies Flat holds for the threads active among them (`slack/timeline.rs`).
//! | `chat.postMessage` | every post, in a conversation or in a thread |
//! | `users.info` | an author's display name when the message does not carry one, and who a mention names |
//! | `users.conversations` | the channel browser |
//! | `conversations.members` | a group DM's members, to name it by them in the channel browser (`slack/groups.rs`) |
//! | `conversations.mark` | clearing unread in Slack, only with `upstream_clear` on (`slack/clear.rs`) |
//!
//! Every call is a form-encoded `POST {api_base}/{method}` with `Authorization: Bearer <token>`.
//! With `upstream_clear` on, `conversations.info` is also read for the account's cursor,
//! `last_read`, before every mark.
//!
//! # Identity mapping
//!
//! The rest of the server is snowflake-ordered, so a Slack `ts` is encoded as a snowflake-shaped
//! [`crate::model::MessageId`] that orders, pages and dates exactly as the `ts` does; see
//! [`ids`]. A whole conversation's channel id is its Slack id, and a channel narrowed to one
//! thread is `"{conversation}~{thread_ts}"`. Thread ids in the thread APIs are root `ts` values.
//!
//! # Mentions
//!
//! A user mention stays `<@U…>` in a message's body, so a reply that quotes it notifies the same
//! person; each read also says who it names, from the `users.info` cache, so the page draws
//! `@Ada Lovelace` and speech says the name (`#318 slack-mention-names`, `slack/mentions.rs`). At
//! most 25 people a read are looked up who have not been already, best effort. A channel mention
//! Slack sends without its name is named from `conversations.info`.
//!
//! # Reactions
//!
//! Each message's reactions come with it, in the same `conversations.history` and
//! `conversations.replies` answers, so reading them costs no call (`#312 slack-reactions`,
//! `slack/reactions.rs`): a standard short name is shown as its emoji, any other as a custom
//! emoji's name, with the count and whether the owner's own `owner_user_id` is among those who
//! reacted. A message Slack lists without them is "not reported". Nothing here adds or removes one.
//!
//! An edited message carries `edited: {user, ts}` in the same answers, read as the message's
//! `edited_timestamp` (`#320 slack-edited-mark`, `Raw::edited_micros`); one without it, or whose
//! `ts` is not one, has none.
//!
//! # What it does not do
//!
//! There is no Events API or Socket Mode connection: new messages arrive through the server's
//! ordinary bounded live polling, which reads a conversation's main timeline and the new replies of
//! the threads rooted on its newest page whose root moved (`#307 slack-reply-live`,
//! `slack/live.rs`). A reply's edit or deletion, and a reply in a thread rooted further back, are
//! read when the thread, Flat or the newest messages are read.
//!
//! An emoji written in a message is Slack's short name between colons, and a standard one is read
//! as the emoji it names (`#321 slack-emoji-shortcodes`, `slack/emoji.rs`); a custom emoji's name
//! stays as written, and nothing is changed in what is posted.
//!
//! A file a message shares is a line of its body, its name and kind linked to Slack's page for it
//! (`#313 slack-file-posts`, `slack/files.rs`). No file, thumbnail or preview is ever fetched.
//!
//! There is no manual "Mark read in Slack" on a message. The one upstream write besides posting is
//! the automatic clear of [`crate::upstream_clear`], off unless the deployment sets
//! `upstream_clear`: it moves the conversation's read cursor forward, never back, and cannot mark
//! a thread read, because Slack offers no way to with this kind of token. Replies inside threads
//! therefore stay unread in Slack, and a channel narrowed to one thread is refused.

mod clear;
mod client;
mod emoji;
mod files;
mod groups;
mod history;
pub mod ids;
mod live;
mod lookup;
mod mentions;
pub mod mrkdwn;
mod range;
mod reactions;
mod threads;
mod timeline;

pub use client::{HttpSlackClient, SLACK_MAX_TEXT_CHARS};
pub use ids::{message_id_from_ts, parse_source, ts_from_message_id};
pub use live::{LIVE_FAILED_READS, LIVE_LAG_READS, LIVE_THREAD_READS, LIVE_TRANSIENT_FAILED_READS};

/// The Slack Web API base used when a deployment does not name a bridge.
pub const DEFAULT_SLACK_API_BASE: &str = "https://slack.com/api";

/// The most characters one post to Slack itself is cut to, unless configured otherwise.
/// `#263 post-split-per-provider`.
///
/// Slack keeps [`SLACK_MAX_TEXT_CHARS`] and truncates past it. A thousand short of that, because
/// Slack does not say how it counts a character, and an emoji is one character here and may be two
/// there.
pub const SLACK_DEFAULT_POST_CHARS: usize = 39_000;

/// Whether `api_base` is Slack's own Web API rather than a bridge in front of it.
#[must_use]
pub fn is_official_slack_api_base(api_base: &str) -> bool {
    reqwest::Url::parse(api_base.trim())
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "slack.com" || host.ends_with(".slack.com"))
}

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
    /// Whether the server may clear a conversation's unread state in Slack's own client, moving
    /// the token's read cursor with `conversations.mark` (`slack/clear.rs`). Off by default.
    pub upstream_clear: bool,
    /// The most characters one post may hold, as Slack is sent it, when configured.
    /// `#263 post-split-per-provider`; see [`SlackConfig::post_limit`].
    pub max_post_chars: Option<usize>,
    /// The most UTF-8 bytes one post may hold, as Slack is sent it, when configured.
    pub max_post_bytes: Option<usize>,
    /// The workspace's own address (`https://acme.slack.com`), when the operator has said it:
    /// where a message's permalink lives, for the page's "Copy link in Slack". `#408
    /// copy-upstream-link`. Unset, the one `auth.test` reports is learned instead; with
    /// neither, no link is formed.
    pub workspace_url: Option<String>,
}

impl SlackConfig {
    /// The most text one post through this connection may carry, counted as Slack is sent it:
    /// with `&`, `<` and `>` escaped ([`mrkdwn::escape_outgoing`]). `#263
    /// post-split-per-provider`.
    ///
    /// Slack itself takes [`SLACK_DEFAULT_POST_CHARS`] unless configured, and never more than
    /// [`SLACK_MAX_TEXT_CHARS`]. A bridge takes what the operator said it accepts
    /// (`max_post_chars`, `max_post_bytes`), and when the operator said nothing, 2000 characters:
    /// the most any bridge has been sent since posts were first split, so an upgrade never sends a
    /// bridge a longer post than it has accepted before.
    #[must_use]
    pub fn post_limit(&self) -> crate::chat::PostLimit {
        let official = is_official_slack_api_base(&self.api_base);
        let stated = self.max_post_chars.is_some() || self.max_post_bytes.is_some();
        let max_chars = match self.max_post_chars {
            Some(max) if official => Some(max.min(SLACK_MAX_TEXT_CHARS)),
            Some(max) => Some(max),
            None if official => Some(SLACK_DEFAULT_POST_CHARS),
            None if !stated => Some(crate::discord::http::DISCORD_MAX_CONTENT_LEN),
            None => None,
        };
        crate::chat::PostLimit {
            max_chars,
            max_bytes: self.max_post_bytes,
            escaping: crate::chat::PostEscaping::HtmlEntities,
        }
    }
}
