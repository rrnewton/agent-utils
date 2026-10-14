//! The newest messages of several threads of one channel in one request to a compatible bridge.
//! remote-ops-audit W14 `#287 bulk-reads`.
//!
//! The trait's default reads several threads one `GET /channels/{id}/timeline?view=thread` at a
//! time. A compatible bridge configured with `thread_reads` answers them all at once with
//! `GET /channels/{id}/threads?ids=…&limit=…` ([`threads_request`]), from one snapshot of the
//! channel; the README's "A bridge's reads of several threads" is the contract. Every bridge
//! without the setting reads several threads with the trait's default.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{HttpDiscordClient, PreparedRequest};
use crate::chat::{ChatError, ThreadAnswers, ThreadAsk};
use crate::model::ChannelId;
use crate::threads::TimelinePage;

/// The most threads one bridge read of several threads names; a longer list is several requests.
pub const BRIDGE_THREAD_IDS: usize = 50;

/// The longest one bridge read of several threads may take: a timeline read's budget, since the
/// bridge answers each thread as its thread view would.
const THREADS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Build a compatible bridge's read of the newest page of each of the threads `ids` of `channel`,
/// every one of `limit` messages. remote-ops-audit W14 `#287 bulk-reads`.
///
/// `GET /channels/{channel}/threads?ids=…&ids=…&limit=…`: the channel percent-encoded as one path
/// segment, one `ids` per thread (opaque ids may hold any character, so they are never joined),
/// and `limit` at least 1. Sent only to a bridge configured with `thread_reads`; the README's "A
/// bridge's reads of several threads" is the contract.
///
/// # Errors
///
/// [`ChatError::Shape`] when the configured API base is not a URL that can take a path.
pub fn threads_request(
    api_base: &str,
    channel: &ChannelId,
    ids: &[String],
    limit: u16,
) -> Result<PreparedRequest, ChatError> {
    let mut url = reqwest::Url::parse(api_base.trim_end_matches('/'))
        .map_err(|_| ChatError::Shape("invalid chat API base".to_owned()))?;
    url.path_segments_mut()
        .map_err(|_| ChatError::Shape("chat API base cannot contain path segments".to_owned()))?
        .pop_if_empty()
        .push("channels")
        .push(channel.as_str())
        .push("threads");
    {
        let mut query = url.query_pairs_mut();
        for id in ids {
            query.append_pair("ids", id);
        }
        query.append_pair("limit", &limit.max(1).to_string());
    }
    Ok(PreparedRequest {
        method: "GET",
        url: url.into(),
        body: None,
    })
}

/// Read a bridge's answer to [`threads_request`]: each thread it answered for, and its page.
/// remote-ops-audit W14 `#287 bulk-reads`.
///
/// Strict, because the request is sent only on the operator's word that the bridge serves it: an
/// answer that is not `{threads: [...]}` fails, and so does an entry of a thread not asked about,
/// a thread answered twice, or a page the thread view's own reader refuses (a message of another
/// channel or thread, a summary of another thread, a continuation that disagrees with `has_more`,
/// a delta nobody asked for). A thread the bridge does not mention is left out, for the caller to
/// read as it would have one at a time.
///
/// # Errors
///
/// [`ChatError::Shape`] for anything but `{threads: [{thread_id, page}]}`, each `page` a thread
/// view page in the timeline wire format, read as above.
pub fn parse_threads(
    value: Value,
    channel: &ChannelId,
    asked: &[String],
    limit: u16,
) -> Result<Vec<(String, TimelinePage)>, ChatError> {
    let shape = |detail: &str| ChatError::Shape(format!("thread read answer: {detail}"));
    let entries = match value {
        Value::Object(mut object) => object.remove("threads"),
        _ => None,
    };
    let Some(Value::Array(entries)) = entries else {
        return Err(shape(
            "expected an object with a threads list; a bridge that does not serve GET \
             /channels/{id}/threads does not read several threads at once, so turn thread_reads \
             off for it",
        ));
    };
    let asked: HashSet<&str> = asked.iter().map(String::as_str).collect();
    let mut answered = HashSet::new();
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Value::Object(mut entry) = entry else {
            return Err(shape("every entry must be an object"));
        };
        let id = match entry.remove("thread_id") {
            Some(Value::String(id)) => id,
            _ => return Err(shape("every entry must name its thread")),
        };
        if !asked.contains(id.as_str()) {
            return Err(shape("an entry of a thread that was not asked about"));
        }
        if !answered.insert(id.clone()) {
            return Err(shape("a thread answered twice"));
        }
        let page = match entry.remove("page") {
            Some(page @ Value::Object(_)) => page,
            _ => return Err(shape("every entry must carry its thread's page")),
        };
        let request = ThreadAsk {
            id: id.clone(),
            newer_than_ms: None,
            limit,
        }
        .request();
        let page = super::threading::parse_bridge_page(page, channel, &request)?;
        if page.delta.is_some() || page.next_after.is_some() {
            return Err(shape(
                "a thread's page carries a forward cursor or a delta nobody asked for",
            ));
        }
        out.push((id, page));
    }
    Ok(out)
}

impl HttpDiscordClient {
    /// A bridge's own read of several threads of `channel`: the asks of the same size are one
    /// request, each distinct thread named once, [`BRIDGE_THREAD_IDS`] threads a request. Each
    /// page is then held to its ask ([`crate::chat::newer_than_trimmed`]). A request that fails,
    /// or whose answer cannot be read, fails each of its threads; a thread the bridge does not
    /// mention is refused in its own place, so its caller reads it as it would have one at a time.
    pub(super) async fn bridge_threads(
        &self,
        channel: &ChannelId,
        asks: &[ThreadAsk],
    ) -> ThreadAnswers {
        let mut alike: Vec<(u16, Vec<String>)> = Vec::new();
        for ask in asks {
            match alike.iter_mut().find(|(limit, _)| *limit == ask.limit) {
                Some((_, ids)) => {
                    if !ids.contains(&ask.id) {
                        ids.push(ask.id.clone());
                    }
                }
                None => alike.push((ask.limit, vec![ask.id.clone()])),
            }
        }
        let mut answers: HashMap<(u16, String), Result<TimelinePage, ChatError>> = HashMap::new();
        for (limit, ids) in &alike {
            for chunk in ids.chunks(BRIDGE_THREAD_IDS) {
                let read = async {
                    let value = self
                        .send_with_timeout(
                            threads_request(&self.api_base, channel, chunk, *limit)?,
                            THREADS_TIMEOUT,
                        )
                        .await?;
                    parse_threads(value, channel, chunk, *limit)
                }
                .await;
                match read {
                    Ok(pages) => {
                        for (id, page) in pages {
                            answers.insert((*limit, id), Ok(page));
                        }
                    }
                    Err(error) => {
                        for id in chunk {
                            answers.insert((*limit, id.clone()), Err(error.clone()));
                        }
                    }
                }
            }
        }
        asks.iter()
            .map(|ask| {
                let answer = answers
                    .get(&(ask.limit, ask.id.clone()))
                    .cloned()
                    .unwrap_or_else(|| {
                        Err(ChatError::Refused(
                            "the bridge gave no read of this thread".to_owned(),
                        ))
                    });
                (
                    ask.id.clone(),
                    answer
                        .map(|page| crate::chat::newer_than_trimmed(page, ask.newer_than_ms))
                        .map_err(|error| error.with_provider(&self.provider_name)),
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
        discord.thread_reads = true;
    }

    const BASE_MS: i64 = 1_787_000_000_000;
    const CHANNEL: &str = "spaces/A";

    /// A message of thread `thread` of the channel in the timeline wire format, created at `ms`.
    fn wire(thread: &str, ms: i64, content: &str) -> Value {
        json!({
            "id": MessageId::at_time_ms(ms).as_str(),
            "channel_id": CHANNEL,
            "content": content,
            "timestamp": crate::clock::iso_from_ms(ms),
            "author": { "id": "7", "username": "bridge", "bot": false },
            "thread": { "id": thread, "root_message_id": null, "is_root": false,
                        "reply_count": 2, "reply_count_exact": true }
        })
    }

    /// Thread `thread`'s page as the thread view answers it: `messages`, and its summary.
    fn page(thread: &str, messages: Vec<Value>) -> Value {
        json!({
            "messages": messages,
            "threads": [],
            "thread": { "id": thread, "root": null, "title": thread, "reply_count": 2,
                        "reply_count_exact": true, "updated_at": "2026-10-13T10:00:00Z" },
            "has_threads": true,
            "has_more": false,
            "next_before": null,
            "notice": null
        })
    }

    fn channel() -> ChannelId {
        ChannelId(CHANNEL.to_owned())
    }

    fn said(answers: &ThreadAnswers, at: usize) -> Vec<String> {
        answers[at]
            .1
            .as_ref()
            .expect("a page")
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect()
    }

    /// The proof for a bridge with `thread_reads`: twenty threads and a repeat are ONE request,
    /// each page what the bridge said of that thread, its staleness included, and a thread it left
    /// out refused in its own place, labelled with the provider.
    #[tokio::test]
    async fn a_bridge_with_thread_reads_answers_every_thread_in_one_request() {
        let ids: Vec<String> = (0..20)
            .map(|n| format!("spaces/A/threads/t{n:02}"))
            .collect();
        let query: Vec<String> = ids
            .iter()
            .map(|id| format!("ids={}", id.replace('/', "%2F")))
            .collect();
        let mut threads: Vec<Value> = ids
            .iter()
            .enumerate()
            .skip(1)
            .map(|(n, id)| {
                json!({"thread_id": id, "page": page(id, vec![
                    wire(id, BASE_MS + i64::try_from(n).expect("small") * 1_000, &format!("in t{n:02}")),
                ])})
            })
            .collect();
        threads[0]["page"]["as_of"] = json!("2026-10-13T09:00:00Z");
        threads[1]["unknown"] = json!("ignored");
        let mut routes = Replies::new();
        ok(
            &mut routes,
            &format!("/channels/spaces%2FA/threads?{}&limit=50", query.join("&")),
            json!({ "threads": threads }),
        );
        let mock = mock(routes, bridge).await;
        assert_eq!(
            mock.client.read_caps(&channel()).many_threads,
            Support::Native
        );
        let mut asks: Vec<ThreadAsk> = ids.iter().map(|id| ThreadAsk::newest(id, 50)).collect();
        asks.push(ThreadAsk::newest(&ids[5], 50));

        let answers = mock.client.fetch_threads(&channel(), &asks).await;

        assert_eq!(
            mock.take().len(),
            1,
            "one request for twenty threads and a repeat"
        );
        assert_eq!(
            answers.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            asks.iter().map(|ask| ask.id.clone()).collect::<Vec<_>>()
        );
        let left_out = answers[0].1.as_ref().expect_err("left out");
        assert!(
            matches!(left_out.cause(), ChatError::Refused(_)),
            "{left_out}"
        );
        assert!(
            matches!(left_out, ChatError::Provider { .. }),
            "labelled with the provider that refused it: {left_out:?}"
        );
        assert_eq!(said(&answers, 1), ["in t01"]);
        assert_eq!(
            answers[1].1.as_ref().expect("a page").as_of.as_deref(),
            Some("2026-10-13T09:00:00Z"),
            "the bridge's word on how current its copy is"
        );
        assert_eq!(said(&answers, 19), ["in t19"]);
        assert_eq!(said(&answers, 20), ["in t05"], "a repeat answered again");
        let summary = answers[7].1.as_ref().expect("a page").thread.clone();
        assert_eq!(summary.map(|s| s.id), Some(ids[7].clone()));
    }

    /// Asks of a different size are a request each; a longer list than one request names is
    /// several; an answer to `newer_than` is held to it as a delta of additions; and a failed
    /// request fails each of its threads only.
    #[tokio::test]
    async fn asks_of_one_size_share_a_request_and_each_answer_is_held_to_its_ask() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/spaces%2FA/threads?ids=t1&limit=3",
            json!({"threads": [{"thread_id": "t1", "page": page("t1", vec![
                wire("t1", BASE_MS, "old"),
                wire("t1", BASE_MS + 1_000, "at the instant"),
                wire("t1", BASE_MS + 2_000, "newer"),
            ])}]}),
        );
        routes.insert(
            "GET /channels/spaces%2FA/threads?ids=t2&limit=50".into(),
            (StatusCode::INTERNAL_SERVER_ERROR, json!({})),
        );
        let mock = mock(routes, bridge).await;
        let answers = mock
            .client
            .fetch_threads(
                &channel(),
                &[
                    ThreadAsk {
                        id: "t1".to_owned(),
                        newer_than_ms: Some(BASE_MS + 1_000),
                        limit: 3,
                    },
                    ThreadAsk::newest("t2", 50),
                ],
            )
            .await;
        assert_eq!(mock.take().len(), 2, "two sizes are two requests");
        assert_eq!(said(&answers, 0), ["newer"]);
        let delta = answers[0].1.as_ref().expect("a page").delta.clone();
        assert_eq!(
            delta.map(|d| (d.complete, d.deleted.len(), d.more)),
            Some((false, 0, false)),
            "additions only: never whole, never a deletion"
        );
        assert!(matches!(
            answers[1].1.as_ref().map_err(ChatError::cause),
            Err(ChatError::Status { status: 500, .. })
        ));

        let many: Vec<ThreadAsk> = (0..120)
            .map(|n| ThreadAsk::newest(format!("t{n}"), 50))
            .collect();
        let answers = mock.client.fetch_threads(&channel(), &many).await;
        let requests = mock.take();
        assert_eq!(requests.len(), 3, "50 threads a request");
        assert!(requests[0].contains("ids=t0&") && requests[0].contains("ids=t49&"));
        assert!(requests[2].contains("ids=t100&") && requests[2].contains("ids=t119&"));
        assert!(
            answers.iter().all(|(_, answer)| answer.is_err()),
            "unserved"
        );
    }

    #[test]
    fn the_strict_reader_refuses_anything_it_cannot_read() {
        let asked = vec!["t1".to_owned(), "t2".to_owned()];
        let good = || page("t1", vec![wire("t1", BASE_MS, "x")]);
        let mut forward = good();
        forward["next_after"] = json!("cursor");
        let mut cut = good();
        cut["has_more"] = json!(true);
        for (value, why) in [
            (
                json!([wire("t1", BASE_MS, "a plain list")]),
                "a plain message list",
            ),
            (json!({"threads": {}}), "threads is not a list"),
            (json!({"threads": [7]}), "an entry that is not an object"),
            (
                json!({"threads": [{"page": good()}]}),
                "an entry with no thread",
            ),
            (
                json!({"threads": [{"thread_id": "t9", "page": good()}]}),
                "a thread not asked about",
            ),
            (
                json!({"threads": [{"thread_id": "t1", "page": good()},
                                   {"thread_id": "t1", "page": good()}]}),
                "a thread answered twice",
            ),
            (json!({"threads": [{"thread_id": "t1"}]}), "no page"),
            (
                json!({"threads": [{"thread_id": "t2", "page": good()}]}),
                "another thread's page",
            ),
            (
                json!({"threads": [{"thread_id": "t1", "page":
                    page("t1", vec![wire("t2", BASE_MS, "x")])}]}),
                "another thread's message",
            ),
            (
                json!({"threads": [{"thread_id": "t1", "page": forward}]}),
                "a forward cursor nobody asked for",
            ),
            (
                json!({"threads": [{"thread_id": "t1", "page": cut}]}),
                "has_more without a continuation",
            ),
        ] {
            assert!(
                matches!(
                    parse_threads(value.clone(), &channel(), &asked, 50),
                    Err(ChatError::Shape(_))
                ),
                "{why}: {value}"
            );
        }
        let read = parse_threads(
            json!({"threads": [{"thread_id": "t2", "page": page("t2", vec![]), "extra": 1}]}),
            &channel(),
            &asked,
            50,
        )
        .expect("an unknown field is ignored");
        assert_eq!(read.len(), 1, "a thread left out is simply not there");
        assert_eq!(read[0].0, "t2");
    }

    #[test]
    fn the_request_names_each_thread_once_and_never_joins_them() {
        let request = threads_request(
            "http://bridge.example/base/",
            &ChannelId("spaces/A".to_owned()),
            &["spaces/A/threads/x,y".to_owned(), "a b".to_owned()],
            0,
        )
        .expect("a request");
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "http://bridge.example/base/channels/spaces%2FA/threads\
             ?ids=spaces%2FA%2Fthreads%2Fx%2Cy&ids=a+b&limit=1"
        );
    }

    /// Off, a bridge says it reads several threads as the default does, and does: one thread
    /// view read per thread, and never `/threads`.
    #[tokio::test]
    async fn without_the_setting_a_bridge_reads_each_thread_as_before() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/spaces%2FA/timeline?view=thread&limit=50&thread_id=t1",
            page("t1", vec![wire("t1", BASE_MS, "one")]),
        );
        ok(
            &mut routes,
            "/channels/spaces%2FA/timeline?view=thread&limit=50&thread_id=t2",
            page("t2", vec![wire("t2", BASE_MS, "two")]),
        );
        let mock = mock(routes, |discord| {
            discord.thread_api = crate::threads::ThreadApi::Bridge;
        })
        .await;
        assert_eq!(
            mock.client.read_caps(&channel()).many_threads,
            Support::Emulated
        );
        let answers = mock
            .client
            .fetch_threads(
                &channel(),
                &[ThreadAsk::newest("t1", 50), ThreadAsk::newest("t2", 50)],
            )
            .await;
        assert_eq!(said(&answers, 0), ["one"]);
        assert_eq!(said(&answers, 1), ["two"]);
        assert_eq!(
            mock.take(),
            [
                "GET /channels/spaces%2FA/timeline?view=thread&limit=50&thread_id=t1",
                "GET /channels/spaces%2FA/timeline?view=thread&limit=50&thread_id=t2",
            ]
        );
    }
}
