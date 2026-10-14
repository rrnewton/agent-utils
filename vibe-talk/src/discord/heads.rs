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
//!
//! A compatible bridge configured with `head_reads` answers `GET /heads?channels=…` itself
//! ([`heads_request`]), from whatever it keeps of its channels; the README's "A bridge's channel
//! heads" is the contract. Such a bridge may also say [`ChannelHead::unchanged_since_ms`], when
//! its own live feed has seen every change since.
//!
//! Either way, only an explicit `null` (`last_message_id`, a bridge's `newest_id`) says a channel
//! has no messages. A channel object or a bridge head that leaves the field out says nothing about
//! the channel's newest message, so that channel's head is refused and it is read as it would be
//! without heads. Fix round 3 of remote-ops-audit W12 `#287 bulk-reads`.

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

/// Discord's channel types that carry a message list of their own: text, a direct message, voice
/// (its text chat), a group direct message, announcement, the three thread types, and stage. A
/// category, a forum and a media channel do not; their `last_message_id` is not a message
/// `GET /channels/{id}/messages` would list.
const MESSAGE_CHANNEL_TYPES: [u64; 9] = [0, 1, 2, 3, 5, 10, 11, 12, 13];

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

/// Why a Discord channel object that leaves `last_message_id` out is no head.
const UNSAID_LAST_MESSAGE: &str = "Discord did not say this channel's last message \
    (last_message_id), so its head proves nothing and the channel is read as it would be without \
    heads";

/// Read a Discord channel object as `expected`'s head, and where its head lives.
///
/// # Errors
///
/// [`ChatError::Shape`] for an object of another channel, or one whose `type` or
/// `last_message_id` cannot be read; [`ChatError::Refused`] for a channel with no message list of
/// its own (a forum, say), whose `last_message_id` is not the poller's to compare, and for one
/// that leaves `last_message_id` out, which says nothing about its newest message.
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
    // Discord documents the field as optional as well as nullable: only `null` says the channel
    // has no messages, and an object that leaves it out says nothing. Fix round 3 of
    // remote-ops-audit W12 `#287 bulk-reads`.
    let newest_id = match value.get("last_message_id") {
        None => return Err(ChatError::Refused(UNSAID_LAST_MESSAGE.to_owned())),
        Some(Value::Null) => None,
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

/// The most channels one bridge head read names; a longer list is several requests.
pub const BRIDGE_HEAD_CHANNELS: usize = 50;

/// The longest message id a bridge head may name, as for its cursors.
const MAX_BRIDGE_ID_LEN: usize = 2048;

/// Build a compatible bridge's head read: the newest message of each of `channels`.
/// remote-ops-audit W12 `#287 bulk-reads`.
///
/// `GET /heads?channels=…&channels=…`, one `channels` per channel (opaque ids may hold any
/// character, so they are never joined). Sent only to a bridge configured with `head_reads`; the
/// README's "A bridge's channel heads" is the contract.
///
/// # Errors
///
/// [`ChatError::Shape`] when the configured API base is not a URL that can take a path.
pub fn heads_request(api_base: &str, channels: &[ChannelId]) -> Result<PreparedRequest, ChatError> {
    let mut request = get(api_base, &["heads"])?;
    let mut url = reqwest::Url::parse(&request.url)
        .map_err(|_| ChatError::Shape("invalid chat API base".to_owned()))?;
    {
        let mut query = url.query_pairs_mut();
        for channel in channels {
            query.append_pair("channels", channel.as_str());
        }
    }
    request.url = url.into();
    Ok(request)
}

/// Read a bridge's answer to [`heads_request`]: each channel it answered for, and its head.
/// remote-ops-audit W12 `#287 bulk-reads`.
///
/// Strict, because the request is sent only on the operator's word that the bridge serves it: an
/// answer that is not `{heads: [...]}` fails, and so does a head of a channel not asked about, a
/// channel answered twice, or a field that cannot be read. A channel the bridge does not mention
/// is left out, for the caller to answer as unread.
///
/// `newest_id` is required: `null` says the channel has no messages, and a head that leaves the
/// field out says nothing about its newest message. Such a head is no head: its channel is
/// answered [`ChatError::Refused`] in its own place, so every caller reads it as it would have
/// without heads, and nothing else the head says is acted on. Fix round 3 of remote-ops-audit
/// W12 `#287 bulk-reads`: it used to be read as `null`, and the live poller then skipped the page
/// of a channel that may well have moved.
///
/// # Errors
///
/// [`ChatError::Shape`] for anything but `{heads: [{channel, newest_id, newest_at, activity_at,
/// unchanged_since, as_of}]}`, every field but `channel` and `newest_id` optional or null, read as
/// above.
pub fn parse_heads(value: &Value, asked: &[ChannelId]) -> Result<HeadAnswers, ChatError> {
    let shape = |detail: &str| ChatError::Shape(format!("channel heads answer: {detail}"));
    let heads = value
        .as_object()
        .and_then(|object| object.get("heads"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            shape(
                "expected an object with a heads list; a bridge that does not serve GET /heads \
                 does not read channel heads, so turn head_reads off for it",
            )
        })?;
    let asked: std::collections::HashSet<&str> = asked.iter().map(ChannelId::as_str).collect();
    let mut answered = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(heads.len());
    for entry in heads {
        let entry = entry
            .as_object()
            .ok_or_else(|| shape("every head must be an object"))?;
        let channel = entry
            .get("channel")
            .and_then(Value::as_str)
            .ok_or_else(|| shape("every head must name its channel"))?;
        if !asked.contains(channel) {
            return Err(shape("a head of a channel that was not asked about"));
        }
        if !answered.insert(channel.to_owned()) {
            return Err(shape("a channel answered twice"));
        }
        // What the head says of the channel's newest message, `None` when it says nothing: left
        // out is not `null`, and only `null` says the channel has none. Fix round 3 of
        // remote-ops-audit W12 `#287 bulk-reads`.
        let said_newest = match entry.get("newest_id") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(id)) if !id.is_empty() && id.len() <= MAX_BRIDGE_ID_LEN => {
                Some(Some(MessageId(id.clone())))
            }
            Some(_) => {
                return Err(shape(
                    "newest_id must be null or a string of 1 to 2048 characters",
                ))
            }
        };
        let instant = |field: &str| -> Result<Option<i64>, ChatError> {
            match entry.get(field) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(text)) => crate::clock::instant_ms(text)
                    .map(Some)
                    .ok_or_else(|| shape(&format!("{field} must be an RFC 3339 instant"))),
                Some(_) => Err(shape(&format!("{field} must be an RFC 3339 instant"))),
            }
        };
        let as_of = match entry.get("as_of") {
            None | Some(Value::Null) => None,
            Some(Value::String(as_of)) => Some(as_of.clone()),
            Some(_) => return Err(shape("as_of must be a string")),
        };
        let head = ChannelHead {
            newest_id: None,
            newest_at_ms: instant("newest_at")?,
            activity_ms: instant("activity_at")?,
            unchanged_since_ms: instant("unchanged_since")?,
            as_of,
        };
        let answer = match said_newest {
            Some(newest_id) => Ok(ChannelHead { newest_id, ..head }),
            None => Err(ChatError::Refused(
                "the bridge's head for this channel did not say its newest message (newest_id), \
                 so it proves nothing and the channel is read as it would be without heads"
                    .to_owned(),
            )),
        };
        out.push((ChannelId(channel.to_owned()), answer));
    }
    Ok(out)
}

fn is_snowflake(raw: &str) -> bool {
    !raw.is_empty() && raw.len() <= 20 && raw.bytes().all(|byte| byte.is_ascii_digit())
}

impl HttpDiscordClient {
    /// A bridge's own heads, [`BRIDGE_HEAD_CHANNELS`] channels a request, each distinct channel
    /// asked once. A request that fails, or whose answer cannot be read, fails each of its
    /// channels; a channel the bridge does not mention is refused in its own place, so its caller
    /// reads it as it would have without heads.
    pub(super) async fn bridge_heads(&self, channels: &[ChannelId]) -> HeadAnswers {
        let mut distinct: Vec<ChannelId> = Vec::new();
        for channel in channels {
            if !distinct.contains(channel) {
                distinct.push(channel.clone());
            }
        }
        let mut answers: HashMap<String, Result<ChannelHead, ChatError>> = HashMap::new();
        for chunk in distinct.chunks(BRIDGE_HEAD_CHANNELS) {
            let read = async {
                let value = self.send(heads_request(&self.api_base, chunk)?).await?;
                parse_heads(&value, chunk)
            }
            .await;
            match read {
                Ok(heads) => {
                    for (channel, head) in heads {
                        answers.insert(channel.0, head);
                    }
                }
                Err(error) => {
                    for channel in chunk {
                        answers.insert(channel.0.clone(), Err(error.clone()));
                    }
                }
            }
        }
        channels
            .iter()
            .map(|channel| {
                let answer = answers.get(channel.as_str()).cloned().unwrap_or_else(|| {
                    Err(ChatError::Refused(
                        "the bridge gave no head for this channel".to_owned(),
                    ))
                });
                (
                    channel.clone(),
                    answer.map_err(|error| error.with_provider(&self.provider_name)),
                )
            })
            .collect()
    }

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
        assert!(
            !mock.client.read_caps(&channels[0]).heads_cover_edits,
            "and they never vouch for edits or deletes"
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

    /// remote-ops-audit §4.1 `#287 bulk-reads`: every request this client sends is counted against
    /// the operation that sent it, which is how diagnostics tell a native head read (a request per
    /// server) from the reads it replaces (a request per channel).
    #[tokio::test]
    async fn each_request_a_head_read_sends_is_counted_against_it() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/101",
            text_channel("101", "9", Some("5001")),
        );
        ok(&mut routes, "/channels/102", text_channel("102", "9", None));
        ok(
            &mut routes,
            "/guilds/9/channels",
            json!([
                text_channel("101", "9", Some("5002")),
                text_channel("102", "9", Some("6001")),
            ]),
        );
        let mock = mock(routes, native).await;
        let channels = ids(&["101", "102"]);
        let (_, first) =
            crate::upstream_profile::counting_calls(mock.client.channel_heads(&channels)).await;
        assert_eq!(mock.take().len(), 2);
        assert_eq!(first, 2, "a request per channel never seen");
        let (_, second) =
            crate::upstream_profile::counting_calls(mock.client.channel_heads(&channels)).await;
        assert_eq!(mock.take(), ["GET /guilds/9/channels"]);
        assert_eq!(second, 1, "then one request for the server");
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

    #[test]
    fn a_channel_object_is_read_as_a_head_and_says_where_the_next_one_lives() {
        let (head, home) =
            native_head(&text_channel("101", "9", Some("5001")), "101").expect("a text channel");
        assert_eq!(head.newest_id, Some(MessageId("5001".into())));
        assert_eq!(home, Home::Server("9".into()));
        let (head, home) =
            native_head(&json!({"id": "7", "type": 1, "last_message_id": null}), "7")
                .expect("a direct message has a message list of its own");
        assert_eq!(head.newest_id, None);
        assert_eq!(home, Home::Alone, "no server list holds it");
        for (value, why) in [
            (text_channel("102", "9", Some("1")), "another channel"),
            (json!({"id": "101", "guild_id": "9"}), "no type"),
            (
                json!({"id": "101", "type": 4, "guild_id": "9"}),
                "a category",
            ),
            (
                json!({"id": "101", "type": 0, "last_message_id": "not-a-snowflake"}),
                "an unreadable last_message_id",
            ),
        ] {
            assert!(native_head(&value, "101").is_err(), "{why}: {value}");
        }
    }

    fn bridge(discord: &mut crate::config::DiscordConfig) {
        discord.thread_api = crate::threads::ThreadApi::Bridge;
        discord.head_reads = true;
    }

    /// The proof for a bridge with `head_reads`: every channel's head is one request, and the
    /// bridge's word on each, its silence and its staleness included, is what comes back.
    #[tokio::test]
    async fn a_bridge_with_head_reads_answers_every_channel_in_one_request() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/heads?channels=spaces%2FA&channels=spaces%2FB&channels=spaces%2FC",
            json!({"heads": [
                {"channel": "spaces/C", "newest_id": "spaces/C/messages/9",
                 "newest_at": "2026-10-13T11:00:00Z", "as_of": "2026-10-13T10:00:00Z"},
                {"channel": "spaces/A", "newest_id": "spaces/A/messages/7",
                 "newest_at": "2026-10-13T11:59:00Z", "activity_at": "2026-10-13T12:00:00Z",
                 "unchanged_since": "2026-10-13T12:00:00Z"},
            ]}),
        );
        let mock = mock(routes, bridge).await;
        let channels = ids(&["spaces/A", "spaces/B", "spaces/C", "spaces/A"]);
        assert_eq!(mock.client.read_caps(&channels[0]).heads, Support::Native);
        assert!(
            mock.client.read_caps(&channels[0]).heads_cover_edits,
            "a bridge's head may say unchanged_since"
        );
        let answers = mock.client.channel_heads(&channels).await;
        assert_eq!(
            mock.take(),
            ["GET /heads?channels=spaces%2FA&channels=spaces%2FB&channels=spaces%2FC"],
            "one request, each distinct channel named once"
        );
        let a = answers[0].1.as_ref().expect("A's head");
        assert_eq!(
            a.newest_id.as_ref().map(MessageId::as_str),
            Some("spaces/A/messages/7")
        );
        assert_eq!(
            a.newest_at_ms,
            crate::clock::instant_ms("2026-10-13T11:59:00Z")
        );
        assert_eq!(
            a.activity_ms,
            crate::clock::instant_ms("2026-10-13T12:00:00Z")
        );
        assert_eq!(
            a.unchanged_since_ms,
            crate::clock::instant_ms("2026-10-13T12:00:00Z")
        );
        assert!(a.is_current());
        assert!(
            matches!(
                answers[1].1.as_ref().map_err(ChatError::cause),
                Err(ChatError::Refused(_))
            ),
            "a channel the bridge did not mention is unread: {:?}",
            answers[1].1
        );
        let c = answers[2].1.as_ref().expect("C's head");
        assert_eq!(c.as_of.as_deref(), Some("2026-10-13T10:00:00Z"));
        assert!(!c.is_current());
        assert_eq!(
            answers[3].1.as_ref().ok(),
            Some(a),
            "a repeat answered again"
        );
    }

    #[tokio::test]
    async fn a_long_list_of_channels_is_a_request_per_fifty() {
        let channels: Vec<ChannelId> = (0..120).map(|n| ChannelId(format!("c{n}"))).collect();
        let mut routes = Replies::new();
        for chunk in channels.chunks(BRIDGE_HEAD_CHANNELS) {
            let query: Vec<String> = chunk.iter().map(|c| format!("channels={c}")).collect();
            let heads: Vec<Value> = chunk
                .iter()
                .map(|c| json!({"channel": c.as_str(), "newest_id": format!("{c}-m")}))
                .collect();
            ok(
                &mut routes,
                &format!("/heads?{}", query.join("&")),
                json!({ "heads": heads }),
            );
        }
        let mock = mock(routes, bridge).await;
        let answers = mock.client.channel_heads(&channels).await;
        assert_eq!(mock.take().len(), 3);
        assert!(answers.iter().all(|(c, head)| head
            .as_ref()
            .is_ok_and(|head| head.newest_id == Some(MessageId(format!("{c}-m"))))));
    }

    #[tokio::test]
    async fn a_bridge_answer_that_cannot_be_read_fails_each_channel_it_was_for() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/heads?channels=a&channels=b",
            json!([{"id": "1", "channel_id": "a", "content": "a plain message list"}]),
        );
        let mock = mock(routes, bridge).await;
        let answers = mock.client.channel_heads(&ids(&["a", "b"])).await;
        assert!(answers.iter().all(|(_, head)| matches!(
            head.as_ref().map_err(ChatError::cause),
            Err(ChatError::Shape(_))
        )));
    }

    #[test]
    fn a_bridge_head_answer_is_read_strictly() {
        let asked = ids(&["a", "b"]);
        let good = parse_heads(
            &json!({"heads": [{"channel": "b", "newest_id": null,
                "newest_at": null, "activity_at": null, "unchanged_since": null, "as_of": null,
                "something_new": true}]}),
            &asked,
        )
        .expect("an empty channel");
        assert_eq!(good.len(), 1);
        assert_eq!(good[0].0.as_str(), "b");
        assert_eq!(good[0].1.as_ref().ok(), Some(&ChannelHead::default()));
        for broken in [
            json!([{"channel": "a"}]),
            json!({"heads": {"channel": "a"}}),
            json!({"heads": ["a"]}),
            json!({"heads": [{"newest_id": "x"}]}),
            json!({"heads": [{"channel": "z"}]}),
            json!({"heads": [{"channel": "a"}, {"channel": "a"}]}),
            json!({"heads": [{"channel": "a", "newest_id": ""}]}),
            json!({"heads": [{"channel": "a", "newest_id": 7}]}),
            json!({"heads": [{"channel": "a", "newest_id": "x".repeat(2049)}]}),
            json!({"heads": [{"channel": "a", "newest_at": "yesterday"}]}),
            json!({"heads": [{"channel": "a", "unchanged_since": 5}]}),
            json!({"heads": [{"channel": "a", "activity_at": "soon"}]}),
            json!({"heads": [{"channel": "a", "as_of": 5}]}),
        ] {
            assert!(
                matches!(parse_heads(&broken, &asked), Err(ChatError::Shape(_))),
                "{broken} was accepted"
            );
        }
    }

    /// Fix round 3 of remote-ops-audit W12 `#287 bulk-reads`: only `newest_id: null` says a channel
    /// has no messages. A head that leaves `newest_id` out says nothing about its newest message,
    /// so it is no head at all: that channel is refused in its own place, whatever else the head
    /// says, and the others of the answer stand.
    #[test]
    fn a_bridge_head_without_newest_id_is_no_head() {
        let asked = ids(&["a", "b", "c"]);
        let answers = parse_heads(
            &json!({"heads": [
                {"channel": "a"},
                {"channel": "b", "newest_id": null},
                {"channel": "c", "newest_at": "2026-10-13T11:59:00Z",
                 "activity_at": "2026-10-13T12:00:00Z", "unchanged_since": "2026-10-13T12:00:00Z"},
            ]}),
            &asked,
        )
        .expect("the answer can be read");
        assert_eq!(answers.len(), 3);
        for at in [0, 2] {
            assert!(
                matches!(&answers[at].1, Err(ChatError::Refused(why)) if why.contains("newest_id")),
                "{}: a head that does not say its newest id proves nothing: {:?}",
                answers[at].0,
                answers[at].1
            );
        }
        assert_eq!(
            answers[1].1.as_ref().ok(),
            Some(&ChannelHead::default()),
            "null says the channel has no messages"
        );
    }

    /// The same for Discord's own channel object: `last_message_id: null` is a channel with no
    /// messages, and an object that leaves the field out says nothing about them.
    #[test]
    fn a_discord_channel_without_last_message_id_is_no_head() {
        let refused = native_head(&json!({"id": "101", "type": 0, "guild_id": "9"}), "101");
        assert!(
            matches!(&refused, Err(ChatError::Refused(why)) if why.contains("last_message_id")),
            "{refused:?}"
        );
        let (empty, _) = native_head(
            &json!({"id": "101", "type": 0, "guild_id": "9", "last_message_id": null}),
            "101",
        )
        .expect("a channel with no messages");
        assert_eq!(empty.newest_id, None);
    }

    /// A bridge's head that does not say its newest id reaches the caller as a refusal of that
    /// channel alone, never as a head that says the channel is empty.
    #[tokio::test]
    async fn a_bridge_head_without_newest_id_reaches_the_caller_as_unread() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/heads?channels=a&channels=b",
            json!({"heads": [{"channel": "a"}, {"channel": "b", "newest_id": "b-1"}]}),
        );
        let mock = mock(routes, bridge).await;
        let answers = mock.client.channel_heads(&ids(&["a", "b"])).await;
        assert!(
            matches!(
                answers[0].1.as_ref().map_err(ChatError::cause),
                Err(ChatError::Refused(_))
            ),
            "{:?}",
            answers[0].1
        );
        assert_eq!(newest(&answers), [None, Some("b-1".into())]);
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
