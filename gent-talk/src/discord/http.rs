//! The real Discord client, over Discord's HTTP API.
//!
//! The parts with judgment in them — which URL, which headers, which body, and how a Discord
//! payload maps onto [`Message`] — are pure functions below, and they are unit-tested. The async
//! wrapper around them is deliberately thin, because it is the only part that cannot be tested
//! without a live token.

use async_trait::async_trait;
use serde_json::json;

use super::{DiscordClient, DiscordError};
use crate::config::{DiscordConfig, Secret};
use crate::model::{ChannelId, Message, MessageId};

/// Discord's own ceiling on `GET /channels/{id}/messages?limit=`.
pub const DISCORD_MAX_LIMIT: u16 = 100;
/// Discord's ceiling on a single message body.
pub const DISCORD_MAX_CONTENT_LEN: usize = 2000;

/// A request, described independently of any HTTP client.
#[derive(Debug, PartialEq, Eq)]
pub struct PreparedRequest {
    /// HTTP method.
    pub method: &'static str,
    /// Fully qualified URL.
    pub url: String,
    /// JSON body, when the method carries one.
    pub body: Option<serde_json::Value>,
}

/// Build the `Authorization` header value for a bot token.
///
/// The `Bot ` prefix is mandatory; without it Discord answers 401 for a perfectly good token.
#[must_use]
pub fn authorization_header(token: &Secret) -> String {
    format!("Bot {}", token.expose())
}

/// Build the request that reads recent messages.
#[must_use]
pub fn fetch_request(api_base: &str, channel: &ChannelId, limit: u16) -> PreparedRequest {
    let limit = limit.clamp(1, DISCORD_MAX_LIMIT);
    PreparedRequest {
        method: "GET",
        url: format!(
            "{}/channels/{}/messages?limit={}",
            api_base.trim_end_matches('/'),
            channel.as_str(),
            limit
        ),
        body: None,
    }
}

/// Build the request that posts a message.
///
/// # Errors
///
/// Returns [`DiscordError::Refused`] when the content is empty or longer than Discord accepts, so
/// the rejection happens here rather than as an opaque 400 from Discord.
pub fn post_request(
    api_base: &str,
    channel: &ChannelId,
    content: &str,
    reply_to: Option<&MessageId>,
) -> Result<PreparedRequest, DiscordError> {
    if content.trim().is_empty() {
        return Err(DiscordError::Refused("message content is empty".to_owned()));
    }
    if content.chars().count() > DISCORD_MAX_CONTENT_LEN {
        return Err(DiscordError::Refused(format!(
            "message content is {} characters; discord accepts at most {DISCORD_MAX_CONTENT_LEN}",
            content.chars().count()
        )));
    }
    let mut body = json!({
        "content": content,
        // Never let a posted message ping everyone because a model repeated "@everyone" back.
        "allowed_mentions": { "parse": [] },
    });
    if let Some(target) = reply_to {
        body["message_reference"] = json!({ "message_id": target.as_str() });
    }
    Ok(PreparedRequest {
        method: "POST",
        url: format!(
            "{}/channels/{}/messages",
            api_base.trim_end_matches('/'),
            channel.as_str()
        ),
        body: Some(body),
    })
}

/// Convert one Discord message object into this server's [`Message`].
///
/// # Errors
///
/// Returns [`DiscordError::Shape`] when a required field is absent or the wrong type.
pub fn parse_message(value: &serde_json::Value) -> Result<Message, DiscordError> {
    let field = |name: &str| -> Result<String, DiscordError> {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| DiscordError::Shape(format!("message is missing a string {name:?}")))
    };
    let author = value
        .get("author")
        .ok_or_else(|| DiscordError::Shape("message is missing an author".to_owned()))?;
    let name = author
        .get("global_name")
        .and_then(serde_json::Value::as_str)
        .or_else(|| author.get("username").and_then(serde_json::Value::as_str))
        .ok_or_else(|| DiscordError::Shape("author has no username".to_owned()))?;
    Ok(Message {
        id: MessageId(field("id")?),
        channel_id: ChannelId(field("channel_id")?),
        author: name.to_owned(),
        author_is_bot: author
            .get("bot")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        timestamp: field("timestamp")?,
        // A message can legitimately have empty content (an embed or an attachment only), so this
        // one is defaulted rather than required.
        content: value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// Convert a Discord message-list payload, normalizing to oldest-first.
///
/// # Errors
///
/// Returns [`DiscordError::Shape`] when the payload is not an array of understandable messages.
pub fn parse_message_list(value: &serde_json::Value) -> Result<Vec<Message>, DiscordError> {
    let items = value
        .as_array()
        .ok_or_else(|| DiscordError::Shape("expected a JSON array of messages".to_owned()))?;
    let mut messages = items
        .iter()
        .map(parse_message)
        .collect::<Result<Vec<_>, _>>()?;
    crate::model::sort_oldest_first(&mut messages);
    Ok(messages)
}

/// A live Discord client.
#[derive(Debug)]
pub struct HttpDiscordClient {
    client: reqwest::Client,
    api_base: String,
    authorization: String,
}

impl HttpDiscordClient {
    /// Build a client from the Discord section of the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`DiscordError::Transport`] when the underlying HTTP client cannot be built.
    pub fn new(config: &DiscordConfig) -> Result<Self, DiscordError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "gent-talk (https://github.com/rrnewton/agent-utils, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| DiscordError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            api_base: config.api_base.clone(),
            authorization: authorization_header(&config.bot_token),
        })
    }

    async fn send(&self, request: PreparedRequest) -> Result<serde_json::Value, DiscordError> {
        let method = match request.method {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            other => return Err(DiscordError::Refused(format!("unsupported method {other}"))),
        };
        let mut builder = self
            .client
            .request(method, &request.url)
            .header(reqwest::header::AUTHORIZATION, &self.authorization);
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let response = builder
            .send()
            .await
            .map_err(|e| DiscordError::Transport(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| DiscordError::Transport(e.to_string()))?;
        if !status.is_success() {
            // Truncate: a Discord error body is short, but an intermediary's error page is not,
            // and this string ends up in a log.
            let mut body = text;
            body.truncate(500);
            return Err(DiscordError::Status {
                status: status.as_u16(),
                body,
            });
        }
        serde_json::from_str(&text).map_err(|e| DiscordError::Shape(e.to_string()))
    }
}

#[async_trait]
impl DiscordClient for HttpDiscordClient {
    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, DiscordError> {
        let value = self
            .send(fetch_request(&self.api_base, channel, limit))
            .await?;
        parse_message_list(&value)
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, DiscordError> {
        let request = post_request(&self.api_base, channel, content, reply_to)?;
        let value = self.send(request).await?;
        parse_message(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://discord.com/api/v10";

    fn channel() -> ChannelId {
        ChannelId("123".to_owned())
    }

    #[test]
    fn authorization_header_carries_the_bot_prefix() {
        let header = authorization_header(&Secret::new("abc"));
        assert_eq!(header, "Bot abc");
    }

    #[test]
    fn fetch_url_matches_the_documented_endpoint() {
        let request = fetch_request(BASE, &channel(), 25);
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "https://discord.com/api/v10/channels/123/messages?limit=25"
        );
        assert!(request.body.is_none());
    }

    #[test]
    fn fetch_limit_is_clamped_to_discord_bounds() {
        assert!(fetch_request(BASE, &channel(), 0).url.ends_with("limit=1"));
        assert!(fetch_request(BASE, &channel(), 9999)
            .url
            .ends_with("limit=100"));
    }

    #[test]
    fn fetch_url_tolerates_a_trailing_slash_on_the_base() {
        let request = fetch_request("https://example.test/api/v10/", &channel(), 5);
        assert_eq!(
            request.url,
            "https://example.test/api/v10/channels/123/messages?limit=5"
        );
    }

    #[test]
    fn post_body_suppresses_mentions() {
        let request =
            post_request(BASE, &channel(), "@everyone deploy is green", None).expect("valid post");
        let body = request.body.expect("post has a body");
        assert_eq!(body["content"], "@everyone deploy is green");
        assert_eq!(
            body["allowed_mentions"]["parse"],
            serde_json::json!([]),
            "a posted message must never be able to ping the server"
        );
        assert!(body.get("message_reference").is_none());
        assert_eq!(
            request.url,
            "https://discord.com/api/v10/channels/123/messages"
        );
    }

    #[test]
    fn post_body_carries_the_reply_target() {
        let request = post_request(BASE, &channel(), "ack", Some(&MessageId("999".to_owned())))
            .expect("valid post");
        let body = request.body.expect("post has a body");
        assert_eq!(body["message_reference"]["message_id"], "999");
    }

    #[test]
    fn empty_and_oversized_posts_are_refused_before_discord_sees_them() {
        assert!(matches!(
            post_request(BASE, &channel(), "   ", None),
            Err(DiscordError::Refused(_))
        ));
        let long = "x".repeat(DISCORD_MAX_CONTENT_LEN + 1);
        assert!(matches!(
            post_request(BASE, &channel(), &long, None),
            Err(DiscordError::Refused(_))
        ));
        let exact = "x".repeat(DISCORD_MAX_CONTENT_LEN);
        assert!(post_request(BASE, &channel(), &exact, None).is_ok());
    }

    fn sample_payload() -> serde_json::Value {
        // Shaped after Discord's documented message object, newest first as Discord returns it.
        serde_json::json!([
          {
            "id": "1000000000000000002",
            "channel_id": "123",
            "content": "second",
            "timestamp": "2026-08-18T12:01:00.000000+00:00",
            "author": { "username": "coder-bot", "global_name": null, "bot": true }
          },
          {
            "id": "1000000000000000001",
            "channel_id": "123",
            "content": "first",
            "timestamp": "2026-08-18T12:00:00.000000+00:00",
            "author": { "username": "rrnewton", "global_name": "Ryan", "bot": false }
          }
        ])
    }

    #[test]
    fn payload_is_mapped_and_reordered_oldest_first() {
        let messages = parse_message_list(&sample_payload()).expect("parses");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "first");
        assert_eq!(messages[0].author, "Ryan", "global_name wins over username");
        assert!(!messages[0].author_is_bot);
        assert_eq!(messages[1].content, "second");
        assert_eq!(
            messages[1].author, "coder-bot",
            "null global_name falls back to username"
        );
        assert!(messages[1].author_is_bot);
        assert_eq!(messages[1].channel_id.as_str(), "123");
    }

    #[test]
    fn a_message_with_no_content_is_kept_not_dropped() {
        let value = serde_json::json!({
            "id": "5", "channel_id": "123", "timestamp": "t",
            "author": { "username": "u" }
        });
        let message = parse_message(&value).expect("attachment-only messages still parse");
        assert_eq!(message.content, "");
        assert!(!message.author_is_bot);
    }

    #[test]
    fn a_malformed_payload_fails_loudly() {
        assert!(matches!(
            parse_message_list(&serde_json::json!({"not": "an array"})),
            Err(DiscordError::Shape(_))
        ));
        assert!(matches!(
            parse_message(&serde_json::json!({"id": "5"})),
            Err(DiscordError::Shape(_))
        ));
    }
}
