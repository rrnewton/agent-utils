//! The newest message of several channels in fewer calls than a read of each.
//! remote-ops-audit W12 `#287 bulk-reads`.
//!
//! Against Discord itself (`thread_api = "native"`) a channel object carries `last_message_id`,
//! and `GET /guilds/{guild}/channels` lists every channel of a server, so the heads of all the
//! configured channels of one server are ONE call. Which server a channel belongs to is learned
//! from the channel itself the first time its head is taken (`GET /channels/{id}`, whose answer is
//! that head too), so a first head read costs what reading each channel would, and every one after
//! it one call per server.
//!
//! `last_message_id` moves only when a message is added: an edit leaves it alone, and a delete
//! may leave it naming a message that is gone. So a Discord head says only that nothing was added
//! after it, never that nothing changed, and never carries
//! [`crate::chat::ChannelHead::unchanged_since_ms`]. The live poller, which reads only additions,
//! may skip a channel whose head has not moved; nothing that must see edits and deletes may.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use serde_json::Value;

use super::{HttpDiscordClient, PreparedRequest};
use crate::chat::{ChannelHead, ChatError, HeadAnswers};
use crate::model::{ChannelId, MessageId};

/// Where a channel's head is read from, once the channel has been seen.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Home {
    /// Its server's channel list.
    Server(String),
    /// Only the channel itself: a thread, which no server channel list holds, or a channel of no
    /// server at all.
    Alone,
}

/// Which server each channel this client has taken a head of belongs to. Learned from the channel
/// itself and never guessed: a channel not yet seen is read on its own.
#[derive(Debug, Default)]
pub(super) struct GuildCache(Mutex<HashMap<String, Home>>);

impl GuildCache {
    fn get(&self, channel: &str) -> Option<Home> {
        self.lock().get(channel).cloned()
    }

    fn set(&self, channel: &str, home: Home) {
        self.lock().insert(channel.to_owned(), home);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Home>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Discord's channel types that carry a message list of their own: text, voice (its text chat),
/// announcement, the three thread types, and stage. A category, a forum and a media channel do
/// not; their `last_message_id` is not a message `GET /channels/{id}/messages` would list.
const MESSAGE_CHANNEL_TYPES: [u64; 7] = [0, 2, 5, 10, 11, 12, 13];

/// Discord's thread channel types, which no server channel list holds.
const THREAD_TYPES: [u64; 3] = [10, 11, 12];

fn shape(detail: &str) -> ChatError {
    ChatError::Shape(format!("channel head: {detail}"))
}

/// A path under the API base, every segment escaped.
fn get(api_base: &str, path: &[&str]) -> Result<PreparedRequest, ChatError> {
    let mut url = reqwest::Url::parse(api_base.trim_end_matches('/'))
        .map_err(|_| ChatError::Shape("invalid chat API base".to_owned()))?;
    url.path_segments_mut()
        .map_err(|_| ChatError::Shape("chat API base cannot contain path segments".to_owned()))?
        .pop_if_empty()
        .extend(path);
    Ok(PreparedRequest {
        method: "GET",
        url: url.into(),
        body: None,
    })
}

/// Read a Discord channel object as `expected`'s head, and where its head lives.
///
/// # Errors
///
/// [`ChatError::Shape`] for an object of another channel, or one whose `type` or
/// `last_message_id` cannot be read; [`ChatError::Refused`] for a channel with no message list of
/// its own (a forum, say), whose `last_message_id` is not the poller's to compare.
fn native_head(value: &Value, expected: &str) -> Result<(ChannelHead, Home), ChatError> {
    if value.get("id").and_then(Value::as_str) != Some(expected) {
        return Err(shape("Discord answered for a different channel"));
    }
    let kind = value
        .get("type")
        .and_then(Value::as_u64)
        .ok_or_else(|| shape("the channel has no type"))?;
    let home = match value.get("guild_id").and_then(Value::as_str) {
        Some(guild) if !THREAD_TYPES.contains(&kind) && is_snowflake(guild) => {
            Home::Server(guild.to_owned())
        }
        _ => Home::Alone,
    };
    if !MESSAGE_CHANNEL_TYPES.contains(&kind) {
        return Err(ChatError::Refused(
            "this channel has no message list of its own to take the newest message of".to_owned(),
        ));
    }
    let newest_id = match value.get("last_message_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if is_snowflake(id) => Some(MessageId(id.clone())),
        Some(_) => return Err(shape("last_message_id is not a snowflake")),
    };
    let head = ChannelHead {
        newest_at_ms: newest_id.as_ref().and_then(MessageId::created_at_ms),
        newest_id,
        ..ChannelHead::default()
    };
    Ok((head, home))
}

fn is_snowflake(raw: &str) -> bool {
    !raw.is_empty() && raw.len() <= 20 && raw.bytes().all(|byte| byte.is_ascii_digit())
}

impl HttpDiscordClient {
    /// Discord's own heads: a channel not seen before (or one no server list holds) is
    /// `GET /channels/{id}`, and every other channel of a server is part of one
    /// `GET /guilds/{guild}/channels`. One request at a time, servers in the order their first
    /// channel was asked. See the module documentation.
    pub(super) async fn native_heads(&self, channels: &[ChannelId]) -> HeadAnswers {
        let mut answers: HashMap<String, Result<ChannelHead, ChatError>> = HashMap::new();
        let mut servers: Vec<String> = Vec::new();
        let mut by_server: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut alone: Vec<String> = Vec::new();
        for channel in channels {
            let id = channel.as_str();
            if answers.contains_key(id)
                || alone.iter().any(|seen| seen == id)
                || by_server.values().flatten().any(|seen| seen == id)
            {
                continue;
            }
            if !is_snowflake(id) {
                answers.insert(
                    id.to_owned(),
                    Err(ChatError::Refused(
                        "this backend requires a numeric native identifier".to_owned(),
                    )),
                );
                continue;
            }
            match self.guilds.get(id) {
                Some(Home::Server(guild)) => {
                    if !by_server.contains_key(&guild) {
                        servers.push(guild.clone());
                    }
                    by_server.entry(guild).or_default().push(id.to_owned());
                }
                Some(Home::Alone) | None => alone.push(id.to_owned()),
            }
        }
        for id in alone {
            let answer = self.native_head_alone(&id).await;
            answers.insert(id, answer);
        }
        for guild in servers {
            let wanted = by_server.remove(&guild).unwrap_or_default();
            match self.server_channels(&guild).await {
                Ok(listed) => {
                    for id in wanted {
                        let answer = match listed.get(&id) {
                            Some(value) => native_head(value, &id).map(|(head, _)| head),
                            // Not in its server's list any more: deleted, or out of this bot's
                            // sight. The channel itself says which.
                            None => self.native_head_alone(&id).await,
                        };
                        answers.insert(id, answer);
                    }
                }
                Err(error) => {
                    for id in wanted {
                        answers.insert(id, Err(error.clone()));
                    }
                }
            }
        }
        channels
            .iter()
            .map(|channel| {
                let answer = answers
                    .get(channel.as_str())
                    .cloned()
                    .unwrap_or_else(|| Err(shape("no head was read for this channel")));
                (
                    channel.clone(),
                    answer.map_err(|error| error.with_provider(&self.provider_name)),
                )
            })
            .collect()
    }

    /// One channel's head from the channel itself, learning where its head lives from now on.
    async fn native_head_alone(&self, id: &str) -> Result<ChannelHead, ChatError> {
        let value = self.send(get(&self.api_base, &["channels", id])?).await?;
        let (head, home) = native_head(&value, id)?;
        self.guilds.set(id, home);
        Ok(head)
    }

    /// Every channel of one server, by id.
    async fn server_channels(&self, guild: &str) -> Result<HashMap<String, Value>, ChatError> {
        let value = self
            .send(get(&self.api_base, &["guilds", guild, "channels"])?)
            .await?;
        let listed = value
            .as_array()
            .ok_or_else(|| shape("a server's channel list is not a list"))?;
        let mut by_id = HashMap::with_capacity(listed.len());
        for channel in listed {
            let id = channel
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| shape("a server's channel list holds a channel with no id"))?;
            by_id.insert(id.to_owned(), channel.clone());
        }
        Ok(by_id)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::{Method, StatusCode, Uri};
    use axum::response::IntoResponse as _;
    use axum::{Json, Router};
    use serde_json::json;

    use super::*;
    use crate::chat::{ChatClient as _, Support};

    type Replies = HashMap<String, (StatusCode, Value)>;

    struct Mock {
        client: HttpDiscordClient,
        seen: Arc<Mutex<Vec<String>>>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for Mock {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl Mock {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.seen.lock().expect("seen"))
        }
    }

    /// A loopback server answering `routes` by `"METHOD /path?query"`, and a client configured by
    /// `configure` against it.
    async fn mock(
        routes: Replies,
        configure: impl FnOnce(&mut crate::config::DiscordConfig),
    ) -> Mock {
        let routes = Arc::new(Mutex::new(routes));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&seen);
        let app = Router::new().fallback(move |method: Method, uri: Uri| {
            let routes = Arc::clone(&routes);
            let captured = Arc::clone(&captured);
            async move {
                let key = format!("{method} {uri}");
                captured.lock().expect("seen").push(key.clone());
                match routes.lock().expect("routes").get(&key) {
                    Some((status, value)) => (*status, Json(value.clone())).into_response(),
                    None => (
                        StatusCode::NOT_FOUND,
                        Json(json!({"missing_mock_route": key})),
                    )
                        .into_response(),
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server");
        });
        let mut config = crate::testing::config();
        let discord = config.discord_mut().expect("discord provider");
        discord.api_base = format!("http://{address}");
        configure(discord);
        let client = HttpDiscordClient::new(discord).expect("client");
        Mock {
            client,
            seen,
            server,
        }
    }

    fn ok(routes: &mut Replies, path: &str, value: Value) {
        routes.insert(format!("GET {path}"), (StatusCode::OK, value));
    }

    fn text_channel(id: &str, guild: &str, last: Option<&str>) -> Value {
        json!({"id": id, "type": 0, "guild_id": guild, "last_message_id": last})
    }

    fn native(discord: &mut crate::config::DiscordConfig) {
        discord.thread_api = crate::threads::ThreadApi::Native;
    }

    fn ids(list: &[&str]) -> Vec<ChannelId> {
        list.iter().map(|id| ChannelId((*id).to_owned())).collect()
    }

    fn newest(answers: &HeadAnswers) -> Vec<Option<String>> {
        answers
            .iter()
            .map(|(_, head)| {
                head.as_ref()
                    .ok()
                    .and_then(|head| head.newest_id.as_ref().map(|id| id.0.clone()))
            })
            .collect()
    }

    /// The proof for Discord: once each channel's server is known, the heads of every configured
    /// channel of a server are one request, whatever else is asked beside them.
    #[tokio::test]
    async fn native_heads_are_one_request_per_server_once_each_channel_has_been_seen() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/101",
            text_channel("101", "9", Some("5001")),
        );
        ok(&mut routes, "/channels/102", text_channel("102", "9", None));
        ok(
            &mut routes,
            "/channels/201",
            text_channel("201", "8", Some("7001")),
        );
        ok(
            &mut routes,
            "/guilds/9/channels",
            json!([
                text_channel("101", "9", Some("5002")),
                text_channel("102", "9", Some("6001")),
                text_channel("199", "9", Some("1")),
                {"id": "100", "type": 4, "guild_id": "9"},
            ]),
        );
        ok(
            &mut routes,
            "/guilds/8/channels",
            json!([text_channel("201", "8", Some("7001"))]),
        );
        let mock = mock(routes, native).await;
        let channels = ids(&["101", "201", "102"]);
        assert_eq!(
            mock.client.read_caps(&channels[0]).heads,
            Support::Native,
            "Discord reads heads itself"
        );

        let first = mock.client.channel_heads(&channels).await;
        assert_eq!(
            newest(&first),
            [Some("5001".into()), Some("7001".into()), None]
        );
        assert_eq!(
            mock.take(),
            [
                "GET /channels/101",
                "GET /channels/201",
                "GET /channels/102"
            ],
            "a channel never seen is read on its own, which is its head too"
        );

        let second = mock.client.channel_heads(&channels).await;
        assert_eq!(
            newest(&second),
            [
                Some("5002".into()),
                Some("7001".into()),
                Some("6001".into())
            ]
        );
        assert_eq!(
            mock.take(),
            ["GET /guilds/9/channels", "GET /guilds/8/channels"],
            "three channels on two servers are two requests"
        );
        let head = second[0].1.as_ref().expect("a head");
        assert_eq!(
            head.newest_at_ms,
            MessageId("5002".into()).created_at_ms(),
            "the instant is the snowflake's"
        );
        assert_eq!(
            head.unchanged_since_ms, None,
            "Discord's head moves only on additions, so it never vouches for edits or deletes"
        );
        assert!(head.is_current());
    }

    #[tokio::test]
    async fn a_channel_missing_from_its_servers_list_is_read_on_its_own() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/101",
            text_channel("101", "9", Some("5001")),
        );
        ok(
            &mut routes,
            "/channels/102",
            text_channel("102", "9", Some("6001")),
        );
        ok(
            &mut routes,
            "/guilds/9/channels",
            json!([text_channel("101", "9", Some("5003"))]),
        );
        let mock = mock(routes, native).await;
        let channels = ids(&["101", "102"]);
        mock.client.channel_heads(&channels).await;
        mock.take();
        let answers = mock.client.channel_heads(&channels).await;
        assert_eq!(newest(&answers), [Some("5003".into()), Some("6001".into())]);
        assert_eq!(mock.take(), ["GET /guilds/9/channels", "GET /channels/102"]);
    }

    #[tokio::test]
    async fn every_failure_is_its_own_channels_answer() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/101",
            text_channel("101", "9", Some("5001")),
        );
        ok(
            &mut routes,
            "/channels/301",
            json!({"id": "301", "type": 15, "guild_id": "9"}),
        );
        ok(
            &mut routes,
            "/channels/401",
            json!({"id": "401", "type": 11, "guild_id": "9", "last_message_id": "4444"}),
        );
        routes.insert(
            "GET /channels/404".into(),
            (
                StatusCode::NOT_FOUND,
                json!({"message": "Unknown Channel", "code": 10003}),
            ),
        );
        routes.insert(
            "GET /guilds/9/channels".into(),
            (StatusCode::INTERNAL_SERVER_ERROR, json!({})),
        );
        let mock = mock(routes, native).await;
        let channels = ids(&["101", "301", "404", "not-a-snowflake", "401"]);
        let first = mock.client.channel_heads(&channels).await;
        assert!(first[0].1.is_ok());
        assert!(
            matches!(
                first[1].1.as_ref().map_err(ChatError::cause),
                Err(ChatError::Refused(_))
            ),
            "a forum has no message list of its own: {:?}",
            first[1].1
        );
        assert!(
            matches!(
                first[2].1.as_ref().map_err(ChatError::cause),
                Err(ChatError::Status { status: 404, .. })
            ),
            "{:?}",
            first[2].1
        );
        assert!(matches!(
            first[3].1.as_ref().map_err(ChatError::cause),
            Err(ChatError::Refused(_))
        ));
        assert_eq!(newest(&first)[4].as_deref(), Some("4444"));
        assert_eq!(
            mock.take(),
            [
                "GET /channels/101",
                "GET /channels/301",
                "GET /channels/404",
                "GET /channels/401"
            ],
            "nothing is asked for an id Discord could not have"
        );

        let second = mock.client.channel_heads(&ids(&["101", "401"])).await;
        assert!(
            matches!(
                second[0].1.as_ref().map_err(ChatError::cause),
                Err(ChatError::Status { status: 500, .. })
            ),
            "a server list that fails fails each of its channels: {:?}",
            second[0].1
        );
        assert_eq!(newest(&second)[1].as_deref(), Some("4444"));
        assert_eq!(
            mock.take(),
            ["GET /channels/401", "GET /guilds/9/channels"],
            "a thread is never looked for in its server's list"
        );
    }

    #[tokio::test]
    async fn a_bridge_and_discord_without_native_threads_read_heads_as_the_default_does() {
        for configure in [
            (|discord: &mut crate::config::DiscordConfig| {
                discord.thread_api = crate::threads::ThreadApi::Off;
            }) as fn(&mut crate::config::DiscordConfig),
            |discord| {
                discord.thread_api = crate::threads::ThreadApi::Bridge;
            },
        ] {
            let mut routes = Replies::new();
            ok(
                &mut routes,
                "/channels/101/messages?limit=1",
                json!([{"id": "5001", "channel_id": "101", "content": "hi",
                    "timestamp": "2026-09-19T10:00:00Z",
                    "author": {"id": "42", "username": "person"}}]),
            );
            let mock = mock(routes, configure).await;
            let channel = ChannelId("101".into());
            assert_eq!(mock.client.read_caps(&channel).heads, Support::Emulated);
            let answers = mock
                .client
                .channel_heads(std::slice::from_ref(&channel))
                .await;
            assert_eq!(newest(&answers), [Some("5001".into())]);
            assert_eq!(mock.take(), ["GET /channels/101/messages?limit=1"]);
        }
    }
}
