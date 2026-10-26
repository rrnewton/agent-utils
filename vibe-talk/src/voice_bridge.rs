//! What a deployment-managed `vibe-talk-v1` bridge says it accepts. `#331 resume-on-websocket`.
//!
//! The browser owns a call's socket, so this server never sees a call's frames. It still has to
//! answer one question about the bridge BEFORE any call: whether a new call can be handed the
//! written record of the earlier one (`#46 conversation-replay`). Settings draws Resuming's switch
//! from that answer, and a switch drawn live over a bridge that cannot take a recap is a control
//! that does nothing — the exact thing the owner reported.
//!
//! # The hello
//!
//! A bridge says what it accepts in the first frame of every session: `session_started` carries
//! `capabilities`, a list of client frame types, and `replay` among them means the bridge takes the
//! `replay` frame and answers it with `replay_ack`. Nothing else is a yes. A bridge that predates
//! the list, lists nothing, or lists other words does not take a recap, and must never be sent
//! one: a server that predates a frame answers it with `error`, which ends its session.
//!
//! So this server asks the way a call would: it opens a session on the bridge, reads
//! `session_started`, sends `quit`, and keeps the list. It asks at startup and again when a page
//! loads client-config and the answer it holds is older than [`HELLO_TTL`] (a failure, older than
//! [`HELLO_FAILURE_TTL`]), one ask at a time, and never on a deployment that does not need the
//! answer — replay off, or the hosted backend ([`needed`]). Each ask opens one session and says
//! nothing in it: no audio, no prompt, no recap.
//!
//! # Which address
//!
//! The server's own route to the bridge: `read_aloud.websocket_url` when it is set, which is its
//! documented use (the application reaches the bridge on loopback while the browser uses a public
//! ingress), and otherwise `conversation.websocket_url` when that is an absolute `ws://` or
//! `wss://` URL. A path means the page's own host, which this server cannot dial the way the page
//! does, so with no other address the answer is a failure that says which setting is missing.
//!
//! # What the answer does not promise
//!
//! A bridge can change between this ask and a call. The page checks the call's own
//! `session_started` as well, and sends a recap only to a session that lists `replay`, so a stale
//! yes here costs a call that says "the voice service takes no recap" — never a recap sent into a
//! session that cannot take it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

use crate::config::{Config, ConversationBackend};
use crate::contract::VibeTalkV1ClientFrame;

/// The word a bridge lists in `session_started.capabilities` when it takes the `replay` frame.
pub const REPLAY: &str = "replay";
/// How long an answer is believed before the next client-config asks again.
pub const HELLO_TTL: Duration = Duration::from_secs(30 * 60);
/// How long a failed ask is believed: a bridge that was briefly down is asked again soon, and one
/// that is always down is not asked on every page load.
pub const HELLO_FAILURE_TTL: Duration = Duration::from_secs(60);
/// Longest the server waits for the bridge to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest the server waits for `session_started` once connected.
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// What a hello came back with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hello {
    /// The bridge started a session; these are the client frame types it listed, possibly none.
    Answered(Vec<String>),
    /// Nothing was heard, and why: no address to ask at, a refused or silent connection, a session
    /// that never started, or an `error` frame in place of `session_started`.
    Failed(String),
}

/// Whether the bridge takes a recap, as far as this server knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaySupport {
    /// It listed `replay`.
    Accepted,
    /// It answered without listing `replay`.
    Declined,
    /// It has not answered: not asked yet, asking, or the ask failed.
    Unknown,
}

/// The kept answer to the hello, and the one ask that may be under way.
#[derive(Debug)]
pub struct BridgeHello {
    url: Option<String>,
    /// `#553 voice-ws-relay-auth`. What the hello presents when it dials `url`.
    credentials: crate::bridge_dial::Credentials,
    kept: Mutex<Option<(Hello, Instant)>>,
    asking: AtomicBool,
    /// The bridge sessions this server opens itself, the hello's among them, each owed one profile
    /// fetch that is never handed a recap. `#512 voice-recap-private-stack`; see
    /// [`crate::voice_recap::OwnSessions`].
    own: Arc<crate::voice_recap::OwnSessions>,
    /// Whether a recap rides in the profile when the bridge declines the frame
    /// (`replay.bridge_fetches_profile`), which changes what a decline means for the log.
    profile_fallback: bool,
}

impl BridgeHello {
    /// Ask at `url`, or nowhere when there is no address this server can dial.
    #[must_use]
    pub fn new(url: Option<String>) -> Self {
        Self {
            url,
            credentials: crate::bridge_dial::Credentials::default(),
            kept: Mutex::new(None),
            asking: AtomicBool::new(false),
            own: Arc::new(crate::voice_recap::OwnSessions::new()),
            profile_fallback: false,
        }
    }

    /// Ask at the server's own route to the configured bridge ([`server_url`]).
    #[must_use]
    pub fn for_config(config: &Config) -> Self {
        // The read-aloud credentials go with read-aloud's own URL only, which `server_url` prefers
        // whenever it is set; `for_read_aloud` is empty otherwise.
        Self {
            profile_fallback: config.replay.bridge_fetches_profile,
            credentials: crate::bridge_dial::Credentials::for_read_aloud(&config.read_aloud),
            ..Self::new(server_url(config))
        }
    }

    /// The address asked at, when there is one.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// The answer kept from the last ask, however old.
    #[must_use]
    pub fn latest(&self) -> Option<Hello> {
        self.lock().as_ref().map(|(hello, _)| hello.clone())
    }

    /// Whether the bridge takes a recap, from [`BridgeHello::latest`].
    #[must_use]
    pub fn replay_support(&self) -> ReplaySupport {
        match self.latest() {
            Some(Hello::Answered(capabilities)) if capabilities.iter().any(|c| c == REPLAY) => {
                ReplaySupport::Accepted
            }
            Some(Hello::Answered(_)) => ReplaySupport::Declined,
            Some(Hello::Failed(_)) | None => ReplaySupport::Unknown,
        }
    }

    /// The bridge sessions this server opens itself: the hello's, and read-aloud's when it is handed
    /// this ([`crate::speech::ConversationSpeech::with_own_sessions`]). `#512
    /// voice-recap-private-stack`: the bridge fetches the profile for each of them as it does for a
    /// call, and a recap armed for the owner's call must not be spent on one.
    #[must_use]
    pub fn own_sessions(&self) -> &Arc<crate::voice_recap::OwnSessions> {
        &self.own
    }

    /// Ask the bridge now, keep what it said, and answer it.
    pub async fn ask(&self) -> Hello {
        let answer = match self.url.as_deref() {
            Some(url) => {
                let said = {
                    let _open = self.own.begin();
                    hello(url, &self.credentials).await
                };
                match said {
                    Ok(capabilities) => Hello::Answered(capabilities),
                    Err(why) => Hello::Failed(why),
                }
            }
            None => Hello::Failed(
                "this server has no address it can reach the voice bridge at: \
                 conversation.websocket_url is not an absolute ws:// or wss:// URL, and \
                 read_aloud.websocket_url is not set"
                    .to_owned(),
            ),
        };
        *self.lock() = Some((answer.clone(), Instant::now()));
        answer
    }

    /// Whether the kept answer has outlived what it may be believed for at `now`, or there is none.
    #[must_use]
    pub fn stale(&self, now: Instant) -> bool {
        match self.lock().as_ref() {
            None => true,
            Some((hello, at)) => {
                let lifetime = match hello {
                    Hello::Answered(_) => HELLO_TTL,
                    Hello::Failed(_) => HELLO_FAILURE_TTL,
                };
                now.saturating_duration_since(*at) >= lifetime
            }
        }
    }

    /// Ask again in the background when the kept answer is [`BridgeHello::stale`], unless an ask is
    /// already under way. Returns at once; what is kept changes when the bridge answers.
    pub fn refresh_if_stale(self: &Arc<Self>) {
        if !self.stale(Instant::now()) || self.asking.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let before = me.latest();
            let answer = me.ask().await;
            me.asking.store(false, Ordering::Release);
            // Said when it changes, not on every ask: a bridge that keeps declining is one warning,
            // not one every half hour.
            if before.as_ref() != Some(&answer) {
                log(&answer, me.profile_fallback);
            }
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(Hello, Instant)>> {
        self.kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Whether this deployment needs the bridge's answer at all: replay is on and the voice backend is
/// a `vibe-talk-v1` bridge. The hosted backend always takes a recap, and with replay off nothing is
/// sent, so neither is ever asked about.
#[must_use]
pub fn needed(config: &Config) -> bool {
    config.replay.enabled && config.conversation.backend == ConversationBackend::WebSocket
}

/// The address this server dials the bridge at: `read_aloud.websocket_url`, then
/// `conversation.websocket_url`, whichever first is an absolute `ws://` or `wss://` URL.
#[must_use]
pub fn server_url(config: &Config) -> Option<String> {
    let absolute = |url: Option<&str>| {
        url.map(str::trim)
            .filter(|url| url.starts_with("ws://") || url.starts_with("wss://"))
            .map(str::to_owned)
    };
    absolute(config.read_aloud.websocket_url.as_deref())
        .or_else(|| absolute(config.conversation.websocket_url.as_deref()))
}

/// Say what the bridge answered, once per ask: a yes at info, anything else as a warning, because
/// it is why Resuming is off on a deployment whose operator turned it on — unless a recap rides in
/// the profile the bridge fetches (`profile_fallback`, `#512 voice-recap-private-stack`), when a
/// decline is only which way the recap goes.
pub fn log(answer: &Hello, profile_fallback: bool) {
    match answer {
        Hello::Answered(capabilities) if capabilities.iter().any(|c| c == REPLAY) => {
            tracing::info!(
                capabilities = %capabilities.join(","),
                "the voice bridge takes a recap (it lists replay): Resuming is offered to the page"
            );
        }
        _ if profile_fallback => tracing::info!(
            "the voice bridge did not say it takes the replay frame, so a new call's recap rides in \
             the voice-agent profile it fetches (replay.bridge_fetches_profile), and each call says \
             whether the bridge took it"
        ),
        Hello::Answered(capabilities) => tracing::warn!(
            capabilities = %capabilities.join(","),
            "replay.enabled is true, but the voice bridge does not list replay among the frames it \
             accepts, so Resuming is reported off to the page and every call starts fresh. A bridge \
             that takes the replay frame lists it in session_started.capabilities."
        ),
        Hello::Failed(why) => tracing::warn!(
            why = %why,
            "replay.enabled is true, but the voice bridge has not said whether it takes a recap, so \
             Resuming is reported off to the page until it does; this server asks again within a \
             minute of the next page load"
        ),
    }
}

/// Open one session on the bridge at `url`, read the `capabilities` its `session_started` lists,
/// and end the session with `quit` without a turn.
///
/// # Errors
///
/// Why nothing was heard, in words: the connection refused or not accepted within ten seconds, no
/// `session_started` within ten seconds of it, an `error` frame in its place, or a close.
pub async fn hello(
    url: &str,
    credentials: &crate::bridge_dial::Credentials,
) -> Result<Vec<String>, String> {
    let mut socket = tokio::time::timeout(
        CONNECT_TIMEOUT,
        crate::bridge_dial::connect(url, credentials),
    )
    .await
    .map_err(|_| "the voice bridge did not accept a connection within 10 seconds".to_owned())?
    .map_err(|error| format!("the voice bridge refused the connection: {error}"))?;
    let ready_by = tokio::time::Instant::now() + READY_TIMEOUT;
    let capabilities = loop {
        let left = ready_by.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(left, socket.next())
            .await
            .map_err(|_| "the voice bridge did not start a session within 10 seconds".to_owned())?;
        match frame {
            Some(Ok(Message::Text(raw))) => {
                let value: serde_json::Value =
                    serde_json::from_str(raw.as_ref()).unwrap_or_default();
                match value.get("type").and_then(serde_json::Value::as_str) {
                    Some("session_started") => break listed(&value),
                    Some("error") => {
                        let said = ["message", "detail"]
                            .iter()
                            .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
                            .unwrap_or("no reason given");
                        return Err(format!(
                            "the voice bridge answered with an error instead of a session: {said}"
                        ));
                    }
                    // `session_started` is always first; anything before it is not this ask's.
                    _ => {}
                }
            }
            Some(Ok(Message::Ping(data))) => {
                let _ = socket.send(Message::Pong(data)).await;
            }
            Some(Ok(Message::Close(_))) | None => {
                return Err("the voice bridge closed before it started a session".to_owned());
            }
            Some(Err(error)) => return Err(format!("the voice bridge connection failed: {error}")),
            Some(Ok(_)) => {}
        }
    };
    // The hello is over: end the session the way a call's Hang up does, with no turn in it.
    let quit = serde_json::to_string(&VibeTalkV1ClientFrame::Quit).unwrap_or_default();
    let _ = socket.send(Message::Text(quit.into())).await;
    let _ = socket.close(None).await;
    Ok(capabilities)
}

/// The words a `session_started` lists under `capabilities`, ignoring anything that is not one.
fn listed(frame: &serde_json::Value) -> Vec<String> {
    frame
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(extra: &str) -> Config {
        let text = format!("{}\n{extra}", crate::testing::config_toml());
        Config::from_toml_and_env(&text, &std::collections::BTreeMap::new()).expect("valid")
    }

    #[test]
    fn the_server_dials_its_own_route_and_never_a_path() {
        let path_only =
            config("[conversation]\nbackend = \"websocket\"\nwebsocket_url = \"/ws?mode=voice\"\n");
        assert_eq!(
            server_url(&path_only),
            None,
            "a path is the page's host, not one this server can dial"
        );

        let absolute = config(
            "[conversation]\nbackend = \"websocket\"\nwebsocket_url = \"wss://voice.example.test/ws\"\n",
        );
        assert_eq!(
            server_url(&absolute).as_deref(),
            Some("wss://voice.example.test/ws")
        );

        let both = config(
            "[conversation]\nbackend = \"websocket\"\nwebsocket_url = \"/ws?mode=voice\"\n\
             [read_aloud]\nwebsocket_url = \"ws://127.0.0.1:9/ws\"\n",
        );
        assert_eq!(
            server_url(&both).as_deref(),
            Some("ws://127.0.0.1:9/ws"),
            "the server-side route wins over the page's"
        );
    }

    #[test]
    fn only_replay_on_a_websocket_backend_needs_the_answer() {
        let websocket = "[conversation]\nbackend = \"websocket\"\nwebsocket_url = \"/ws\"\n";
        assert!(needed(&config(&format!(
            "{websocket}[replay]\nenabled = true\n"
        ))));
        assert!(
            !needed(&config(websocket)),
            "replay off sends nothing, so asks nothing"
        );
        assert!(
            !needed(&config("[replay]\nenabled = true\n")),
            "the hosted backend always can"
        );
    }

    #[test]
    fn only_a_listed_replay_is_a_yes_and_a_failure_is_not_a_no() {
        let kept = |hello: Hello| {
            let bridge = BridgeHello::new(None);
            *bridge.lock() = Some((hello, Instant::now()));
            bridge.replay_support()
        };
        assert_eq!(
            BridgeHello::new(None).replay_support(),
            ReplaySupport::Unknown
        );
        assert_eq!(
            kept(Hello::Answered(vec!["replay".into()])),
            ReplaySupport::Accepted
        );
        assert_eq!(
            kept(Hello::Answered(vec!["Replay".into(), "replays".into()])),
            ReplaySupport::Declined
        );
        assert_eq!(kept(Hello::Answered(Vec::new())), ReplaySupport::Declined);
        assert_eq!(kept(Hello::Failed("down".into())), ReplaySupport::Unknown);
    }

    #[test]
    fn an_answer_is_kept_longer_than_a_failure() {
        let bridge = BridgeHello::new(None);
        let start = Instant::now();
        assert!(bridge.stale(start), "nothing kept is stale");
        *bridge.lock() = Some((Hello::Answered(Vec::new()), start));
        assert!(!bridge.stale(start + HELLO_FAILURE_TTL));
        assert!(bridge.stale(start + HELLO_TTL));
        *bridge.lock() = Some((Hello::Failed("down".into()), start));
        assert!(!bridge.stale(start + Duration::from_secs(1)));
        assert!(bridge.stale(start + HELLO_FAILURE_TTL));
    }

    #[tokio::test]
    async fn with_no_address_the_ask_fails_and_names_the_settings() {
        let bridge = BridgeHello::new(None);
        let Hello::Failed(why) = bridge.ask().await else {
            panic!("an ask with no address answered");
        };
        assert!(why.contains("read_aloud.websocket_url"), "{why}");
        assert_eq!(bridge.replay_support(), ReplaySupport::Unknown);
    }

    #[test]
    fn listed_ignores_what_is_not_a_word() {
        let frame = serde_json::json!({"type": "session_started", "capabilities": ["replay", 3, null, "x"]});
        assert_eq!(listed(&frame), vec!["replay".to_owned(), "x".to_owned()]);
        assert!(listed(&serde_json::json!({"type": "session_started"})).is_empty());
    }
}
