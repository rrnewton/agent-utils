//! The HTTP surface: a small JSON API, plus the web app that consumes it.
//!
//! Route policy, stated once so it is checkable:
//!
//! * `/healthz` is unauthenticated and returns nothing about the configuration.
//! * The static web app is unauthenticated — it is public code, and it holds no secret. It asks
//!   the operator for an API token at runtime.
//! * Everything under `/api/` requires a bearer token, and the write scope is required for
//!   anything that reaches Discord with a message — and for minting a signed conversation URL,
//!   which is a credential for talking to an agent that can itself post.
//! * **No read-scope credential ever writes anything durable.** The read token is the one pasted
//!   into a hosted voice agent, so it is the least trusted thing that holds a credential here;
//!   every durable write — appending a turn, moving a read mark, filing a cached summary —
//!   requires the write scope. `/summary` is readable at read scope and is served from the cache
//!   when there is a hit, but a read-scope caller's generated summary is NOT filed; see
//!   [`crate::ops::summarize_message`]. One durable change can follow a read-scope request, and
//!   it writes nothing: a confirmed loss of access erases cached copies whoever noticed it. A
//!   provider's refusal of a channel read is confirmed by the server's own read before the
//!   message cache erases what it held of it (`#208 persistent-cache`; see
//!   [`crate::cache::access`]).
//! * The write scope is ALSO required to READ a transcript. A stored transcript is the owner's
//!   own speech plus whatever channel text was read aloud to him, which is not a read of a
//!   channel he already allowlisted. The one durable read a read-scope token may make is
//!   `/inbox`, because how far HE has read is what the agent has to be able to say out loud.
//! * **Who is asking is settled before what they asked.** Every `/api/` handler takes its
//!   credential as its first extractor after `State` — [`api::ReadScope`], [`api::WriteScope`],
//!   or on the two ticket routes [`api::SpeechTicket`] — so `Path`, `Query`, `Json` and `Bytes`
//!   never answer a caller without one. The two live-ingest routes are the exceptions in form,
//!   not in effect: each takes the whole request and authenticates before it reads the body.
//!   `tests/scope_first.rs` checks every route registered here against an inventory, both in this
//!   file's source and by probing the router for methods the inventory does not list.
//! * **Routing does not answer first either.** [`bearer_layer`] refuses an `/api/` request that
//!   carries neither browser token with the same 401 a route gives — status, headers and body —
//!   BEFORE routing, so an unknown path or an unserved method does not tell an anonymous caller
//!   which routes exist or which methods they serve. The two live-ingest
//!   routes and the two ticket routes, which carry their own credentials, are exempt for their own
//!   method only. So is the pairing-code redemption, whose code is its credential
//!   (`#369 pairing-link`).
//! * Every request — routed or not, authorized or not — leaves exactly ONE line in the access
//!   log at INFO. See [`crate::access`] for why that is load-bearing rather than nice to have.
//!   **The one line for `/stream` is written at ATTACH, with `millis=0`**, because the middleware
//!   returns as soon as the status is known and a streaming body has not started yet. A stream
//!   held open for an hour still logs zero; see [`api::stream`].
//! * `/api/v1/channels/{id}/stream` is a long-lived Server-Sent Events response and is otherwise
//!   an ordinary read: same bearer token, same read scope, same channel allowlist. A stream is the
//!   easiest thing here to leave accidentally open, so it is deliberately NOT a special case.
//! * **A read-scope caller is never sent a pairing code.** A pairing link redeems for the write
//!   token, and one pasted into a bridged channel would otherwise reach every read-scope caller.
//!   [`redact_layer`] rewrites every `/api/` and `/mcp` answer to the read token as it leaves
//!   (`#369 pairing-link`).
//! * `/api/v1/voice-socket` is the voice socket relayed to a bridge only this server reaches
//!   (`#553 voice-ws-relay-auth`). Its credential is a single-use ticket from the page's own
//!   voice-session call, or the voice-only token, which opens this route and no other; the door in
//!   front of routing exempts it for `GET` alone. See [`voice_socket`].
//! * `/mcp` is the Streamable HTTP MCP endpoint. It requires a bearer token too, and answers a
//!   credential-less caller with a bland 401 before it reads the body at all.

pub mod access_layer;
pub mod api;
pub mod assets;
pub mod bearer_layer;
pub mod redact_layer;
pub mod voice_socket;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::compression::predicate::{Predicate, SizeAbove};
use tower_http::compression::{CompressionLayer, CompressionLevel};

use crate::state::AppState;

/// Build the whole router.
///
/// The routes sit inside an outer router whose only job is to carry the two middlewares, because
/// `Router::layer` does not run before routing: it wraps each matched route and each fallback, so
/// an unserved method's 405 path would still add its `allow` header to `bearer_layer`'s 401 and
/// list the methods a path serves. Wrapped from outside, the bearer check sees every request before
/// routing does, and the access log still sees every request.
pub fn router(state: AppState) -> Router {
    Router::new()
        .fallback_service(routes(state.clone()))
        // Innermost: what a read-scope caller is sent carries no pairing code, whichever route
        // produced it. `#369 pairing-link`; see `redact_layer`.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            redact_layer::redact_for_read_scope,
        ))
        // Inside the access log, so a request refused here still leaves its one line.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            bearer_layer::require_bearer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state,
            access_layer::log_requests,
        ))
        // Outermost, so the access log and the bearer check see the request as sent and the body
        // is compressed last. Brotli or gzip, whichever the client offers first by preference; what
        // is compressed, and how hard, is [`compression`]'s business.
        //
        // And so OUTSIDE the bearer check, which `#217 markdown-blocks` found matters: compression
        // adds a `vary` header to every answer it sees, so a 401 it never saw would lack the one a
        // route's answer carries and tell an anonymous caller which paths reached a route.
        // `tests/scope_first.rs` compares the two.
        .layer(compression())
}

/// Response compression, gzip or brotli as the browser offers. `#217 markdown-blocks`.
///
/// Nothing the server sent used to be compressed. The page's script is one large file, fetched
/// whole on every cold load and again after every release (`assets` names it by its content hash
/// so a browser may keep it in between), and every channel read is JSON whose message bodies now
/// come with their rendered HTML beside them. Both are text that compresses to a fraction of
/// itself, and the reader is on a phone's connection.
///
/// BY CONTENT TYPE, and an allowlist rather than a list of exceptions: the page, its script and
/// styles, its manifest, and JSON. Not the live stream, which must reach the page event by event
/// rather than when a compressor's buffer fills, and not read-aloud audio, which is compressed
/// already and is forwarded as it arrives, so an encoder buffering it would only delay the first
/// sound. Anything added later stays uncompressed until it is named here, which costs bytes but
/// never breaks a stream. Bodies under 32 bytes are left alone, as the default predicate did.
///
/// Quality 5, for both encoders. The default for brotli is its maximum, eleven, which spends tens
/// of milliseconds of CPU on every request for the script, compressed afresh each time; five keeps
/// most of the saving for a small fraction of that. A request without `Accept-Encoding` is
/// answered exactly as before.
fn compression() -> CompressionLayer<impl Predicate> {
    let compressible = |_: axum::http::StatusCode,
                        _: axum::http::Version,
                        headers: &axum::http::HeaderMap,
                        _: &axum::http::Extensions| {
        compressible_type(
            headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or(""),
        )
    };
    CompressionLayer::new()
        .gzip(true)
        .br(true)
        .quality(CompressionLevel::Precise(5))
        .compress_when(SizeAbove::default().and(compressible))
}

/// Whether a response of this `Content-Type` is compressed. See [`compression`].
#[must_use]
pub fn compressible_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "text/html"
            | "text/css"
            | "text/javascript"
            | "application/javascript"
            | "application/json"
            | "application/manifest+json"
    )
}

/// Every route. `tests/scope_first.rs` reads the routes from this function's source.
fn routes(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(api::healthz))
        // The static web app. One handler, which finds the file by path and decides what a
        // browser may keep of it: see `assets` for why the shell revalidates and the files it
        // names are immutable under their hashed URLs.
        .route("/", get(assets::serve))
        // `#302 root-is-the-app`. The app's old address, and the same with a trailing slash: a
        // 302 to `/` keeping the query, so a bookmark or an app installed from there still opens.
        .route("/voice", get(assets::forward_to_root))
        .route("/voice/", get(assets::forward_to_root))
        .route("/voice.js", get(assets::serve))
        .route("/contract.js", get(assets::serve))
        .route("/voice.css", get(assets::serve))
        .route("/style.css", get(assets::serve))
        .route("/manifest.webmanifest", get(assets::serve))
        .route("/icons/{name}", get(assets::serve))
        .route("/api/v1/channels", get(api::list_channels))
        // `#190 voice-agent-tools`. READ scope: every listed channel's last few hours in one call,
        // the route behind the `recent_activity` tool. It reads what `/digest` reads, once per
        // channel, and writes nothing.
        .route("/api/v1/activity", get(api::activity))
        .route("/api/v1/agent-tools", get(api::agent_tools))
        .route("/api/v1/client-config", get(api::client_config))
        .route("/api/v1/live/events", post(api::ingest_event))
        .route("/api/v1/live/hints", post(api::ingest_hint))
        // `#452 slack-live-ingest`. The ingest credential, not a browser token: an adapter posts
        // its provider's stream records here, and asks which conversations to tail.
        .route(
            "/api/v1/live/tail",
            post(api::ingest_tail).get(api::tail_channels),
        )
        // READ scope. It reports configuration health and never a credential, it writes nothing
        // durable, and it always answers 200 — the report is the answer, so a failing check is
        // not an HTTP failure. See `api::diagnostics`.
        .route("/api/v1/diagnostics", get(api::diagnostics))
        .route("/api/v1/signed-url", get(api::signed_url))
        // `#552 wit-tts-provider`. READ scope: public catalogue names for the Settings
        // voice picker; no credential crosses. See `api::read_aloud_voices`.
        .route("/api/v1/read-aloud/voices", get(api::read_aloud_voices))
        // `#369 pairing-link`. WRITE scope to mint a single-use code, because the code redeems for
        // the write token. Redeeming takes NO bearer token: the code in the body is the
        // credential, every refusal is one 410, and failures are limited per client address. See
        // `crate::pairing`.
        .route("/api/v1/pair", post(api::mint_pairing))
        .route("/api/v1/pair/redeem", post(api::redeem_pairing))
        .route("/api/v1/voice-session", get(api::voice_session))
        // `#553 voice-ws-relay-auth`. The voice socket, relayed to `conversation.relay_upstream`:
        // a page's single-use ticket or the voice-only token, never a browser token.
        .route("/api/v1/voice-socket", get(voice_socket::relay))
        // `#14 voice-agent-prompt`. READ scope: the prompt is public text from this repository and
        // the tool list is filtered to the caller's own scope, so it discloses nothing the same
        // credential could not learn from `tools/list`. A private bridge fetches it at session open.
        .route("/api/v1/voice-agent", get(api::voice_agent))
        // `#512 voice-recap-private-stack`. WRITE scope: a recap is the transcript, rendered. The
        // page arms one before it opens a call's socket; the bridge's next fetch of the profile
        // above takes it into the prompt; the page asks what became of it and settles it.
        .route(
            "/api/v1/voice-recap",
            post(api::arm_voice_recap).delete(api::withdraw_any_voice_recap),
        )
        .route(
            "/api/v1/voice-recap/{id}",
            get(api::voice_recap_state).delete(api::withdraw_voice_recap),
        )
        // `#11 voice-connect-latency`. Content-free startup phases, one log line per call. WRITE
        // scope, the same credential that opened the session.
        .route("/api/v1/voice-timing", post(api::voice_timing))
        // `voice-unresponsive-signal`. The page's content-free verdict that the voice service stopped
        // answering, one bounded log line per cause per call. WRITE scope, like the timing record.
        .route(
            "/api/v1/voice-health",
            post(api::voice_health)
                .layer(DefaultBodyLimit::max(crate::voice_health::MAX_BODY_BYTES)),
        )
        // `#268 voice-call-drops`. How a call that opened ended — hang-up, the service's error,
        // the service closing it, a lost connection, the background — one content-free line per
        // call. WRITE scope, like the timing and health records.
        .route(
            "/api/v1/voice-end",
            post(api::voice_end).layer(DefaultBodyLimit::max(crate::voice_health::MAX_BODY_BYTES)),
        )
        // `#511 voice-explicit-send-only`. What the page heard the user say, as each turn grows, so
        // the voice agent's ask to send can be checked against the user's own words. WRITE scope;
        // held in memory for minutes, never logged or stored.
        .route(
            "/api/v1/voice-heard",
            post(api::voice_heard).layer(DefaultBodyLimit::max(crate::send_intent::MAX_BODY_BYTES)),
        )
        .route("/api/v1/channels/{channel_id}/messages", get(api::messages))
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}",
            get(api::message_by_id),
        )
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}/summary",
            get(api::message_summary),
        )
        .route("/api/v1/channels/{channel_id}/page", get(api::page))
        .route("/api/v1/channels/{channel_id}/timeline", get(api::timeline))
        .route("/api/v1/channels/{channel_id}/count", get(api::count))
        .route("/api/v1/channels/{channel_id}/digest", get(api::digest))
        .route("/api/v1/channels/{channel_id}/resolve", post(api::resolve))
        .route("/api/v1/channels/{channel_id}/reply", post(api::reply))
        .route(
            "/api/v1/channels/{channel_id}/reply/outcome",
            post(api::reply_outcome),
        )
        .route("/api/v1/channels/{channel_id}/ask", post(api::ask))
        // `#34 voice-chat-write-confirm`. WRITE scope, all of them, and NO tool reaches the last
        // two: `post_reply` proposes through the first, and only the person the bot speaks for —
        // the voice page's Send, or a bridge's attributed yes — commits. See `api::commit_post`.
        .route(
            "/api/v1/post-proposals",
            get(api::pending_post).post(api::propose_post),
        )
        .route("/api/v1/post-proposals/commit", post(api::commit_post))
        .route("/api/v1/post-proposals/cancel", post(api::cancel_post))
        .route("/api/v1/channels/{channel_id}/stream", get(api::stream))
        // `#337 voice-relay-silent`. READ scope, like each channel's own stream: every listed
        // channel's arrivals in one stream, for a page in a call. See `api::stream_all`.
        .route("/api/v1/live/stream", get(api::stream_all))
        // Durable state. The conversation routes all require the WRITE scope, reads included —
        // see the block comment above them in `api` for why a transcript is more sensitive than
        // a digest. `/inbox` is a read; moving a mark is a write, because it mutates state
        // another device reads back.
        .route(
            "/api/v1/conversations",
            get(api::list_conversations).delete(api::forget_conversations),
        )
        .route(
            "/api/v1/conversations/{conversation_id}",
            get(api::conversation).delete(api::forget_conversation),
        )
        .route(
            "/api/v1/conversations/{conversation_id}/turns",
            post(api::append_turn),
        )
        // `#46 conversation-replay`. A READ of a transcript, rendered — so it takes the write
        // scope like every other conversation route, for the reason stated above them.
        .route(
            "/api/v1/conversations/{conversation_id}/replay",
            get(api::replay),
        )
        // The record as one continuous thing, newest first, a page at a time. Beside the
        // conversation routes rather than under them because it is not about A conversation: the
        // newest forty turns routinely span two calls. Write scope, same as its neighbours.
        .route("/api/v1/transcript", get(api::transcript))
        // The operator's erase, and the only COMPLETE one over HTTP: the conversation routes
        // clear transcripts and say so, this one clears everything the store holds.
        .route("/api/v1/storage", axum::routing::delete(api::purge_storage))
        // `#208 persistent-cache`. The owner's switch for the message cache: PUT turns it off
        // (marker first, then the erase) or on again, DELETE erases it and keeps it on. WRITE scope
        // both, and NO MCP TOOL for either — the saved copy is the owner's to keep or throw away,
        // for the `#39` reason below. Its status rides on client-config.
        .route(
            "/api/v1/message-cache",
            axum::routing::put(api::set_message_cache).delete(api::erase_message_cache),
        )
        .route("/api/v1/inbox", get(api::inbox))
        // `#50 todo-view`. The channel filtered down to what has not been dealt with, and the two
        // acts that change that. READ scope to look, WRITE scope to change — the same split every
        // durable write on this server takes, and the reason it is not "read scope because it is
        // only a flag" is that the flag outlives the process and another device reads it back.
        // `#50` read-aloud. POST because it spends money at a vendor; see `api::speak`.
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}/speak",
            post(api::speak),
        )
        // Read-aloud, prepared ahead of the tap. `prepare` resolves everything on screen ONCE when
        // the mode is turned on; `play` is what a tap actually costs, and it is a map lookup and
        // then the vendor's bytes forwarded as they arrive.
        //
        // `play` and its timing callback are deliberately OUTSIDE the bearer-authenticated shape
        // of every other route. An `<audio src>` cannot send an `Authorization` header, so the
        // random ticket in the path is the intentional capability for both: it grants one already
        // resolved audio read and the right to append content-free clocks to that same read. A
        // guessed ticket gets the same 404 from either route. See `speech_tickets` for why that is
        // a smaller grant than it looks.
        .route(
            "/api/v1/channels/{channel_id}/speech/prepare",
            post(api::prepare_speech),
        )
        .route("/api/v1/speech/{ticket}", get(api::play_speech))
        .route(
            "/api/v1/speech/{ticket}/timing",
            post(api::speech_playback_timing),
        )
        // `#548 cloud-read-failures`. A read the page stopped because the voice failed it.
        .route(
            "/api/v1/speech/{ticket}/failure",
            post(api::speech_playback_failure).get(api::speech_stream_failure),
        )
        // Adding a channel from inside the app. WRITE scope, and deliberately NO MCP tool: this
        // widens what the bridge can read, and what it can say. See `api::add_channel`.
        .route("/api/v1/channels", post(api::add_channel))
        // `#19 channel-browser`. WRITE scope too: it lists every channel the bridge can see, and
        // exists only to choose one to add. See `api::channel_directory`.
        .route("/api/v1/channel-directory", get(api::channel_directory))
        // `#199 removable-config-channels`. DELETE takes any channel off the list: an added one is
        // forgotten, a configured one is hidden, because this server never writes its file. The
        // second route shows a hidden one again. WRITE scope both, and NO MCP TOOL for either —
        // the list is the owner's, for the `#39` reason below. See `api::remove_channel`.
        .route("/api/v1/channels/{channel_id}", delete(api::remove_channel))
        .route(
            "/api/v1/channels/{channel_id}/hidden",
            delete(api::show_channel),
        )
        .route("/api/v1/channels/{channel_id}/todo", get(api::todo))
        .route("/api/v1/channels/{channel_id}/dismiss", post(api::dismiss))
        .route("/api/v1/channels/{channel_id}/restore", post(api::restore))
        // `#549 own-read-stored`. "Apply to existing messages": the owner's own messages already
        // seen and not Done, marked read once, on request. WRITE scope, and no MCP tool.
        .route("/api/v1/own-read/apply", post(api::apply_own_read))
        .route(
            "/api/v1/channels/{channel_id}/read",
            post(api::mark_read).delete(api::forget_read_mark),
        )
        .route(
            "/api/v1/channels/{channel_id}/upstream-read",
            post(api::mark_read_upstream),
        )
        // `#232 upstream-mark-read`. Whether this server clears the channel's unread state at its
        // provider as it ingests messages. READ scope to see it, WRITE scope to change it, and NO
        // MCP TOOL: on, it marks people's messages read in the owner's own client too.
        .route(
            "/api/v1/channels/{channel_id}/upstream-clear",
            get(api::upstream_clear).put(api::set_upstream_clear),
        )
        // `#602 history-settings-and-limits`. How much of the channel's history the message cache
        // holds and how its walk is going (READ scope); its own history mode (WRITE scope); and
        // "read this one next", which search's "Search further back" calls (WRITE scope). NO MCP
        // TOOL for any of the writes: how much the server reads of the owner's channels is his.
        .route(
            "/api/v1/channels/{channel_id}/history",
            get(api::channel_history).put(api::set_channel_history),
        )
        .route(
            "/api/v1/channels/{channel_id}/history/prioritize",
            post(api::prioritize_channel_history).delete(api::cancel_channel_history_priority),
        )
        // `#253 upstream-clear-polish`: Settings' "Mark read now".
        .route(
            "/api/v1/channels/{channel_id}/upstream-clear/now",
            post(api::upstream_clear_now),
        )
        // `#39 channel-alias`. The operator's own local name for a channel. WRITE scope both
        // ways, because it outlives the process — and NO MCP TOOL at all, which is the part that
        // actually keeps a model from renaming the channels it reports on. See `api::set_alias`
        // for why the scope alone would not. Reading an alias needs no route of its own: it rides
        // on every channel this server hands back.
        .route(
            "/api/v1/channels/{channel_id}/alias",
            axum::routing::put(api::set_alias).delete(api::clear_alias),
        )
        // `#196 auto-read-noise`. The rules that decide which messages are read automatically,
        // and the one per-message override. WRITE scope both, because they outlive the process
        // and every device reads them back — and NO MCP TOOL for either, for the `#39` reason
        // above made sharper: a model that could add a rule could hide messages from the owner
        // it reports to. Reading the rules rides on client-config.
        .route(
            "/api/v1/noise-rules",
            axum::routing::put(api::set_noise_rules),
        )
        // `#264 voice-send-approval`. Whether the voice agent asks before it sends. READ scope to
        // see it, WRITE scope to change it — and NO MCP TOOL, because a model that could turn its
        // own confirmation off could then post whatever a message it read asked for.
        .route(
            "/api/v1/voice-settings",
            get(api::voice_settings).put(api::set_voice_settings),
        )
        // `#457 settings-server-side`. The owner's synced settings: everything in
        // Settings outside Screen and keys, kept here for all his devices. READ scope
        // to see them, WRITE scope to change them, and NO MCP TOOL, for the `#39`
        // reason on the alias above made sharper — these decide how his messages are
        // marked and shown.
        .route(
            "/api/v1/client-settings",
            get(api::client_settings).put(api::set_client_settings),
        )
        // `#272 voice-agent-instructions`. The owner's additional instructions and his edited
        // system prompt. READ scope to see them, as the assembled prompt is; WRITE scope to change
        // them — and NO MCP TOOL, because a model that could rewrite its own instructions could
        // drop the rules about untrusted chat text. The body is bounded at the texts' own limits.
        .route(
            "/api/v1/voice-prompt",
            get(api::voice_prompt)
                .put(api::set_voice_prompt)
                .layer(DefaultBodyLimit::max(api::VOICE_PROMPT_BODY_BYTES)),
        )
        // `#303 voice-post-prefix`. The tag the server puts before every message the voice agent
        // posts. READ scope to see it, WRITE scope to save it (PUT) or go back to the default
        // (DELETE) — and NO MCP TOOL, because a model that could take its own tag off could pass
        // its words off as the owner's.
        .route(
            "/api/v1/voice-post-prefix",
            get(api::voice_post_prefix)
                .put(api::set_voice_post_prefix)
                .delete(api::reset_voice_post_prefix),
        )
        .route(
            "/api/v1/channels/{channel_id}/not-noise",
            post(api::not_noise),
        )
        // `#206 pin-message`. The owner's own pinned messages. READ scope to list them, WRITE
        // scope to pin and unpin — PUT and DELETE on the one message, idempotent both ways — and
        // NO MCP TOOL for any of it, for the `#39` reason above. A timeline read carries the
        // list's revision, so the list itself is read on entering a channel and when it changed.
        .route("/api/v1/channels/{channel_id}/pins", get(api::pins))
        .route(
            "/api/v1/channels/{channel_id}/pins/{message_id}",
            axum::routing::put(api::pin_message).delete(api::unpin_message),
        )
        // `#240 reading-bookmark`. The owner's place in a channel's backlog, one per channel. READ
        // scope to see it, WRITE scope to move or clear it — a PUT or DELETE that may carry
        // `if_revision`, refused 409 when another device moved it first — and NO MCP TOOL, for the
        // `#39` reason above. Every channel read carries it too, so this GET is seldom needed.
        .route(
            "/api/v1/channels/{channel_id}/bookmark",
            get(api::bookmark)
                .put(api::set_bookmark)
                .delete(api::clear_bookmark),
        )
        // `#427 server-thread-hues`. A browser offers the thread colours it remembers from
        // before the server decided them, once, for the threads the server has not coloured.
        // WRITE scope and NO MCP TOOL, for the `#39` reason above. Reading the colours needs no
        // route of its own: every message and thread summary carries its thread's hue.
        .route(
            "/api/v1/channels/{channel_id}/thread-hues",
            post(api::seed_thread_hues)
                .layer(DefaultBodyLimit::max(api::THREAD_HUE_SEED_BODY_BYTES)),
        )
        // One path, three methods: POST carries the whole protocol, and GET/DELETE — which exist
        // in the spec for server-initiated streams and session teardown — are refused plainly
        // because this endpoint is stateless and has nothing to push.
        .route(
            crate::mcp::transport::MCP_PATH,
            post(crate::mcp::transport::post).fallback(crate::mcp::transport::method_not_allowed),
        )
        .fallback(api::not_found)
        .with_state(state)
}
