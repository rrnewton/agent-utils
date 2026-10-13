//! The newest messages of several channels in one request to a compatible bridge.
//! remote-ops-audit W13 `#287 bulk-reads`.
//!
//! The trait's default reads several channels one `GET /channels/{id}/messages?limit=` at a time.
//! A compatible bridge configured with `recent_many_reads` answers them all at once with
//! `GET /recent?channels=…&limit=…` ([`recent_request`]), from whatever it keeps of its channels;
//! the README's "A bridge's recent reads of several channels" is the contract. Discord itself has
//! no such request, so `thread_api = "native"`, and every bridge without the setting, read several
//! channels with the trait's default.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{parse_message_list, HttpDiscordClient, PreparedRequest, DISCORD_MAX_LIMIT};
use crate::chat::{ChatError, RecentAnswers, RecentAsk, RecentRead};
use crate::model::ChannelId;

/// The most channels one bridge read of several channels names; a longer list is several
/// requests.
pub const BRIDGE_RECENT_CHANNELS: usize = 50;

/// The size a bridge read of several channels asks each for: what `GET /channels/{id}/messages`
/// takes, 1 to 100, so a read of several asks exactly what a read of each would have.
fn clamped(ask: RecentAsk) -> RecentAsk {
    RecentAsk {
        limit: ask.limit.clamp(1, DISCORD_MAX_LIMIT),
        ..ask
    }
}

/// Build a compatible bridge's read of the newest messages of each of `channels`, every one asked
/// for the same `ask`. remote-ops-audit W13 `#287 bulk-reads`.
///
/// `GET /recent?channels=…&channels=…&limit=…&since=…`: one `channels` per channel (opaque ids may
/// hold any character, so they are never joined), `limit` 1 to 100, and `since` an RFC 3339
/// instant, only when the ask has one. Sent only to a bridge configured with `recent_many_reads`;
/// the README's "A bridge's recent reads of several channels" is the contract.
///
/// # Errors
///
/// [`ChatError::Shape`] when the configured API base is not a URL that can take a path.
pub fn recent_request(
    api_base: &str,
    channels: &[ChannelId],
    ask: RecentAsk,
) -> Result<PreparedRequest, ChatError> {
    let ask = clamped(ask);
    let mut url = reqwest::Url::parse(api_base.trim_end_matches('/'))
        .map_err(|_| ChatError::Shape("invalid chat API base".to_owned()))?;
    url.path_segments_mut()
        .map_err(|_| ChatError::Shape("chat API base cannot contain path segments".to_owned()))?
        .pop_if_empty()
        .push("recent");
    {
        let mut query = url.query_pairs_mut();
        for channel in channels {
            query.append_pair("channels", channel.as_str());
        }
        query.append_pair("limit", &ask.limit.to_string());
        if let Some(since_ms) = ask.since_ms {
            query.append_pair("since", &crate::clock::iso_from_ms(since_ms));
        }
    }
    Ok(PreparedRequest {
        method: "GET",
        url: url.into(),
        body: None,
    })
}

/// Read a bridge's answer to [`recent_request`]: each channel it answered for, and what it read.
/// remote-ops-audit W13 `#287 bulk-reads`.
///
/// Strict, because the request is sent only on the operator's word that the bridge serves it: an
/// answer that is not `{recent: [...]}` fails, and so does an entry of a channel not asked about, a
/// channel answered twice, a message of another channel, a message listed twice, or a field that
/// cannot be read. A channel the bridge does not mention is left out, for the caller to read as it
/// would have one at a time.
///
/// Each read is held to the ask as Slack's and a range read's answers are: its newest `limit`
/// messages ([`DISCORD_MAX_LIMIT`] at most), oldest first, none created before `since`
/// ([`crate::chat::since_trimmed`]). One that lost a message to that is not `exhausted`, whatever
/// the bridge said.
///
/// # Errors
///
/// [`ChatError::Shape`] for anything but `{recent: [{channel, messages, exhausted, as_of}]}`, with
/// `messages` in the message wire format `/messages` uses and `exhausted` and `as_of` optional or
/// null, read as above.
pub fn parse_recent(
    value: &Value,
    asked: &[ChannelId],
    ask: RecentAsk,
) -> Result<Vec<(ChannelId, RecentRead)>, ChatError> {
    let ask = clamped(ask);
    let shape = |detail: &str| ChatError::Shape(format!("recent read answer: {detail}"));
    let entries = value
        .as_object()
        .and_then(|object| object.get("recent"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            shape(
                "expected an object with a recent list; a bridge that does not serve GET /recent \
                 does not read several channels at once, so turn recent_many_reads off for it",
            )
        })?;
    let asked: HashSet<&str> = asked.iter().map(ChannelId::as_str).collect();
    let mut answered = HashSet::new();
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let entry = entry
            .as_object()
            .ok_or_else(|| shape("every entry must be an object"))?;
        let channel = entry
            .get("channel")
            .and_then(Value::as_str)
            .ok_or_else(|| shape("every entry must name its channel"))?;
        if !asked.contains(channel) {
            return Err(shape("an entry of a channel that was not asked about"));
        }
        if !answered.insert(channel.to_owned()) {
            return Err(shape("a channel answered twice"));
        }
        let channel = ChannelId(channel.to_owned());
        let list = entry
            .get("messages")
            .filter(|messages| messages.is_array())
            .ok_or_else(|| shape("every entry must carry a list of messages"))?;
        let mut messages = parse_message_list(list)?;
        let mut ids = HashSet::new();
        for message in &messages {
            if message.channel_id != channel {
                return Err(shape("a message of a different channel"));
            }
            if !ids.insert(message.id.0.clone()) {
                return Err(shape("a message listed twice"));
            }
        }
        let exhausted = match entry.get("exhausted") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(exhausted)) => *exhausted,
            Some(_) => return Err(shape("exhausted must be true or false")),
        };
        let as_of = match entry.get("as_of") {
            None | Some(Value::Null) => None,
            Some(Value::String(as_of)) => Some(as_of.clone()),
            Some(_) => return Err(shape("as_of must be a string")),
        };
        let excess = messages.len().saturating_sub(usize::from(ask.limit));
        messages.drain(..excess);
        let read = RecentRead {
            messages,
            as_of,
            exhausted: exhausted && excess == 0,
            notice: None,
        };
        out.push((channel, crate::chat::since_trimmed(read, ask.since_ms)));
    }
    Ok(out)
}

impl HttpDiscordClient {
    /// A bridge's own read of several channels: the asks that read alike (the same size and
    /// `since`) are one request, each distinct channel named once, [`BRIDGE_RECENT_CHANNELS`]
    /// channels a request. A request that fails, or whose answer cannot be read, fails each of its
    /// channels; a channel the bridge does not mention is refused in its own place, so its caller
    /// reads it as it would have one at a time.
    pub(super) async fn bridge_recent_many(
        &self,
        asks: &[(ChannelId, RecentAsk)],
    ) -> RecentAnswers {
        let mut alike: Vec<(RecentAsk, Vec<ChannelId>)> = Vec::new();
        for (channel, ask) in asks {
            let ask = clamped(*ask);
            match alike.iter_mut().find(|(other, _)| *other == ask) {
                Some((_, channels)) => {
                    if !channels.contains(channel) {
                        channels.push(channel.clone());
                    }
                }
                None => alike.push((ask, vec![channel.clone()])),
            }
        }
        let mut answers: HashMap<(RecentAsk, String), Result<RecentRead, ChatError>> =
            HashMap::new();
        for (ask, channels) in &alike {
            for chunk in channels.chunks(BRIDGE_RECENT_CHANNELS) {
                let read = async {
                    let value = self
                        .send(recent_request(&self.api_base, chunk, *ask)?)
                        .await?;
                    parse_recent(&value, chunk, *ask)
                }
                .await;
                match read {
                    Ok(reads) => {
                        for (channel, read) in reads {
                            answers.insert((*ask, channel.0), Ok(read));
                        }
                    }
                    Err(error) => {
                        for channel in chunk {
                            answers.insert((*ask, channel.0.clone()), Err(error.clone()));
                        }
                    }
                }
            }
        }
        asks.iter()
            .map(|(channel, ask)| {
                let answer = answers
                    .get(&(clamped(*ask), channel.0.clone()))
                    .cloned()
                    .unwrap_or_else(|| {
                        Err(ChatError::Refused(
                            "the bridge gave no recent read for this channel".to_owned(),
                        ))
                    });
                (
                    channel.clone(),
                    answer.map_err(|error| error.with_provider(&self.provider_name)),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::http::{Method, StatusCode, Uri};
    use axum::response::IntoResponse as _;
    use axum::{Json, Router};
    use serde_json::json;

    use super::*;
    use crate::chat::{ChatClient as _, Support};
    use crate::model::MessageId;

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

    fn bridge(discord: &mut crate::config::DiscordConfig) {
        discord.thread_api = crate::threads::ThreadApi::Bridge;
        discord.recent_many_reads = true;
    }

    const BASE_MS: i64 = 1_787_000_000_000;

    /// A message of `channel` in the wire format `/messages` uses, created at `ms`.
    fn wire(channel: &str, ms: i64, content: &str) -> Value {
        json!({
            "id": MessageId::at_time_ms(ms).as_str(),
            "channel_id": channel,
            "content": content,
            "timestamp": crate::clock::iso_from_ms(ms),
            "author": { "id": "7", "username": "bridge", "bot": false }
        })
    }

    fn ids(list: &[&str]) -> Vec<ChannelId> {
        list.iter().map(|id| ChannelId((*id).to_owned())).collect()
    }

    fn said(answers: &RecentAnswers, at: usize) -> Vec<String> {
        answers[at]
            .1
            .as_ref()
            .expect("a read")
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect()
    }

    /// The proof for a bridge with `recent_many_reads`: five channels are one request, and the
    /// bridge's word on each, its silence and its staleness included, is what comes back.
    #[tokio::test]
    async fn a_bridge_with_recent_many_reads_answers_every_channel_in_one_request() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/recent?channels=spaces%2FA&channels=spaces%2FB&channels=spaces%2FC\
             &channels=spaces%2FD&channels=spaces%2FE&limit=20",
            json!({"recent": [
                {"channel": "spaces/C", "messages": [wire("spaces/C", BASE_MS, "c one")],
                 "as_of": "2026-10-13T10:00:00Z"},
                {"channel": "spaces/A", "messages": [
                    wire("spaces/A", BASE_MS + 2_000, "a two"),
                    wire("spaces/A", BASE_MS + 1_000, "a one"),
                 ], "exhausted": true, "unknown": "ignored"},
                {"channel": "spaces/B", "messages": [], "as_of": null},
                {"channel": "spaces/E", "messages": [wire("spaces/E", BASE_MS, "e one")]},
            ]}),
        );
        let mock = mock(routes, bridge).await;
        let channels = ids(&["spaces/A", "spaces/B", "spaces/C", "spaces/D", "spaces/E"]);
        assert_eq!(
            mock.client.read_caps(&channels[0]).many_channels,
            Support::Native
        );
        let mut asks: Vec<(ChannelId, RecentAsk)> = channels
            .iter()
            .map(|channel| (channel.clone(), RecentAsk::newest(20)))
            .collect();
        asks.push((channels[0].clone(), RecentAsk::newest(20)));

        let answers = mock.client.fetch_recent_many(&asks).await;

        assert_eq!(
            mock.take().len(),
            1,
            "one request for five channels and a repeat"
        );
        assert_eq!(
            answers.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
            asks.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>()
        );
        assert_eq!(said(&answers, 0), ["a one", "a two"], "oldest first");
        assert!(answers[0].1.as_ref().expect("a read").exhausted);
        assert!(said(&answers, 1).is_empty());
        assert_eq!(said(&answers, 2), ["c one"]);
        assert_eq!(
            answers[2].1.as_ref().expect("a read").as_of.as_deref(),
            Some("2026-10-13T10:00:00Z"),
            "the bridge's word on how current its copy is"
        );
        let left_out = answers[3].1.as_ref().expect_err("left out");
        assert!(
            matches!(left_out.cause(), ChatError::Refused(_)),
            "{left_out}"
        );
        assert!(
            matches!(left_out, ChatError::Provider { .. }),
            "labelled with the provider that refused it: {left_out:?}"
        );
        assert_eq!(said(&answers, 4), ["e one"]);
        assert_eq!(
            said(&answers, 5),
            ["a one", "a two"],
            "a repeat answered again"
        );
    }

    /// Asks that differ in size or `since` are a request each; a longer list than one request
    /// names is several; a `since` is sent and held to; an answer longer than asked keeps its
    /// newest; and a failed request fails each of its channels only.
    #[tokio::test]
    async fn asks_that_read_alike_share_a_request_and_each_answer_is_held_to_its_ask() {
        let mut routes = Replies::new();
        let since = crate::clock::iso_from_ms(BASE_MS + 1_000).replace(':', "%3A");
        ok(
            &mut routes,
            &format!("/recent?channels=1&limit=2&since={since}"),
            json!({"recent": [{"channel": "1", "exhausted": true, "messages": [
                wire("1", BASE_MS, "too old"),
                wire("1", BASE_MS + 1_000, "at the since"),
                wire("1", BASE_MS + 2_000, "newest"),
            ]}]}),
        );
        ok(
            &mut routes,
            "/recent?channels=2&limit=100",
            json!({"recent": [{"channel": "2", "exhausted": true, "messages": [
                wire("2", BASE_MS, "two")
            ]}]}),
        );
        routes.insert(
            "GET /recent?channels=3&limit=5".into(),
            (StatusCode::INTERNAL_SERVER_ERROR, json!({})),
        );
        let mock = mock(routes, bridge).await;
        let answers = mock
            .client
            .fetch_recent_many(&[
                (
                    ChannelId("1".into()),
                    RecentAsk {
                        limit: 2,
                        since_ms: Some(BASE_MS + 1_000),
                    },
                ),
                (ChannelId("2".into()), RecentAsk::newest(500)),
                (ChannelId("3".into()), RecentAsk::newest(5)),
            ])
            .await;
        assert_eq!(
            mock.take().len(),
            3,
            "three different asks are three requests"
        );
        assert_eq!(said(&answers, 0), ["at the since", "newest"]);
        assert!(
            !answers[0].1.as_ref().expect("a read").exhausted,
            "a read cut to its ask is not the whole channel"
        );
        assert_eq!(said(&answers, 1), ["two"], "a size past 100 asks for 100");
        assert!(answers[1].1.as_ref().expect("a read").exhausted);
        assert!(matches!(
            answers[2].1.as_ref().map_err(ChatError::cause),
            Err(ChatError::Status { status: 500, .. })
        ));

        let many: Vec<ChannelId> = (0..120).map(|n| ChannelId(format!("c{n}"))).collect();
        let asks: Vec<(ChannelId, RecentAsk)> = many
            .iter()
            .map(|channel| (channel.clone(), RecentAsk::newest(20)))
            .collect();
        let answers = mock.client.fetch_recent_many(&asks).await;
        let requests = mock.take();
        assert_eq!(requests.len(), 3, "50 channels a request");
        assert!(requests[0].contains("channels=c0&") && requests[0].contains("channels=c49&"));
        assert!(requests[2].contains("channels=c100&") && requests[2].contains("channels=c119&"));
        assert!(
            answers.iter().all(|(_, answer)| answer.is_err()),
            "unserved"
        );
    }

    #[test]
    fn the_strict_reader_refuses_anything_it_cannot_read() {
        let asked = ids(&["1", "2"]);
        let ask = RecentAsk::newest(20);
        for (value, why) in [
            (
                json!([wire("1", BASE_MS, "a plain list")]),
                "a plain message list",
            ),
            (json!({"recent": {}}), "recent is not a list"),
            (json!({"recent": [7]}), "an entry that is not an object"),
            (
                json!({"recent": [{"messages": []}]}),
                "an entry with no channel",
            ),
            (
                json!({"recent": [{"channel": "9", "messages": []}]}),
                "a channel not asked about",
            ),
            (
                json!({"recent": [{"channel": "1", "messages": []},
                                  {"channel": "1", "messages": []}]}),
                "a channel answered twice",
            ),
            (json!({"recent": [{"channel": "1"}]}), "no messages"),
            (
                json!({"recent": [{"channel": "1", "messages": [wire("2", BASE_MS, "x")]}]}),
                "another channel's message",
            ),
            (
                json!({"recent": [{"channel": "1", "messages": [
                    wire("1", BASE_MS, "x"), wire("1", BASE_MS, "x")]}]}),
                "a message listed twice",
            ),
            (
                json!({"recent": [{"channel": "1", "messages": [{"id": "1"}]}]}),
                "a message that cannot be read",
            ),
            (
                json!({"recent": [{"channel": "1", "messages": [], "exhausted": "yes"}]}),
                "exhausted that is not a boolean",
            ),
            (
                json!({"recent": [{"channel": "1", "messages": [], "as_of": 7}]}),
                "as_of that is not a string",
            ),
        ] {
            assert!(
                matches!(parse_recent(&value, &asked, ask), Err(ChatError::Shape(_))),
                "{why}: {value}"
            );
        }
        let read = parse_recent(
            &json!({"recent": [{"channel": "2", "messages": [], "extra": {"x": 1}}]}),
            &asked,
            ask,
        )
        .expect("an unknown field is ignored");
        assert_eq!(read.len(), 1, "a channel left out is simply not there");
        assert_eq!(read[0].1, RecentRead::default());
    }

    #[test]
    fn the_request_names_each_channel_once_and_never_joins_them() {
        let request = recent_request(
            "http://bridge.example/base/",
            &ids(&["spaces/A,B", "x y"]),
            RecentAsk {
                limit: 0,
                since_ms: Some(BASE_MS),
            },
        )
        .expect("a request");
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            format!(
                "http://bridge.example/base/recent?channels=spaces%2FA%2CB&channels=x+y&limit=1\
                 &since={}",
                crate::clock::iso_from_ms(BASE_MS).replace(':', "%3A")
            )
        );
    }

    /// Off, a bridge says it reads several channels as the default does, and does: one
    /// `/messages` per channel, and never `/recent`.
    #[tokio::test]
    async fn without_the_setting_a_bridge_reads_each_channel_as_before() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/1/messages?limit=20",
            json!([wire("1", BASE_MS, "one")]),
        );
        ok(
            &mut routes,
            "/channels/2/messages?limit=20",
            json!([wire("2", BASE_MS, "two")]),
        );
        let mock = mock(routes, |discord| {
            discord.thread_api = crate::threads::ThreadApi::Bridge;
        })
        .await;
        let channels = ids(&["1", "2"]);
        assert_eq!(
            mock.client.read_caps(&channels[0]).many_channels,
            Support::Emulated
        );
        let answers = mock
            .client
            .fetch_recent_many(&[
                (channels[0].clone(), RecentAsk::newest(20)),
                (channels[1].clone(), RecentAsk::newest(20)),
            ])
            .await;
        assert_eq!(said(&answers, 0), ["one"]);
        assert_eq!(said(&answers, 1), ["two"]);
        assert_eq!(
            mock.take(),
            [
                "GET /channels/1/messages?limit=20",
                "GET /channels/2/messages?limit=20"
            ]
        );
    }
}
