//! The checked-in wire contract matches the Rust types that produce it.
//!
//! `contract/vibe-talk.schema.json` is generated from `src/contract.rs`, and the browser's
//! declarations and validators are generated from that schema. These tests fail when either the
//! schema or the sample payloads beside it are stale, so a change to a wire type cannot land
//! without the reviewable diff it implies. `make -C vibe-talk contract` rewrites both (it runs
//! these tests with `VIBE_TALK_UPDATE_CONTRACT=1`) and then regenerates the browser files.
//!
//! The samples are serialized by the real types, so `tests/js/contract.test.mjs` can prove the
//! generated validators accept exactly what this server sends — every union variant, and every
//! optional field both present and omitted.

use std::path::PathBuf;

use serde_json::{json, Value};
use vibe_talk::contract::{
    ApiErrorBody, ClientConfigResponse, CommittedPostResponse, LiveDeleteEvent, LiveDelivery,
    LiveMessageEvent, LiveResetEvent, NoiseRules, PendingPost, PendingPostResponse,
    PinChangeResponse, PinsResponse, ProviderDescription, ReplyDismissals, TimelineResponse,
    TokenScope, TranscriptRole, VibeTalkV1ClientFrame, VibeTalkV1ServerFrame,
};
use vibe_talk::conversation::{VoiceDescription, VoiceSession};
use vibe_talk::model::{ChannelId, ChannelInfo, Message, MessageId, Reaction, UserId};
use vibe_talk::speech::{Description, Playback};
use vibe_talk::store::{Pin, PinSnapshot};
use vibe_talk::threads::{MessageThread, ThreadSummary, TimelineDelta, TimelinePage, TimelineView};

fn contract_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contract")
}

/// Compare `text` with the checked-in file, or rewrite the file when asked to.
fn check_or_update(name: &str, text: &str) {
    let path = contract_dir().join(name);
    if std::env::var_os("VIBE_TALK_UPDATE_CONTRACT").is_some_and(|v| v == "1") {
        std::fs::write(&path, text).expect("write the regenerated contract file");
        return;
    }
    let checked_in = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        checked_in == text,
        "{} is stale: the Rust wire types changed without regenerating the contract. Run \
         `make -C vibe-talk contract` and commit the result.",
        path.display()
    );
}

fn channel() -> ChannelInfo {
    ChannelInfo {
        id: ChannelId("100".into()),
        label: "general".into(),
        writable: true,
        alias: None,
        added: false,
        provider: Some("discord".into()),
    }
}

fn message(id: &str) -> Message {
    Message {
        thread: None,
        id: MessageId(id.into()),
        channel_id: ChannelId("100".into()),
        author: "someone".into(),
        author_id: UserId("7".into()),
        author_is_bot: false,
        timestamp: "2026-09-25T12:00:00+00:00".into(),
        spoken_time: String::new(),
        reply_to: None,
        content: "hello".into(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
    }
}

fn threaded_message(id: &str) -> Message {
    Message {
        thread: Some(MessageThread {
            id: "t1".into(),
            root_message_id: Some(MessageId("200".into())),
            is_root: false,
            reply_count: Some(3),
            reply_count_exact: true,
        }),
        spoken_time: "noon".into(),
        reply_to: Some(MessageId("200".into())),
        spoken_content: "hello, spoken".into(),
        // `#196 auto-read-noise`: present on one sample and absent on the other, so the
        // generated validators are proven against both.
        noise: true,
        // `#217 markdown-blocks`, the same way.
        content_html: "<p><em>hello</em></p>\n".into(),
        // `#219 emoji-reactions`: a standard emoji and a custom one here, none on the plain
        // sample, and an empty list on a third, because absent and empty are different answers.
        reactions: Some(vec![
            Reaction {
                emoji: "👀".into(),
                custom: false,
                custom_id: None,
                count: 1,
            },
            Reaction {
                emoji: "party-parrot".into(),
                custom: true,
                custom_id: Some("112233".into()),
                count: 2,
            },
        ]),
        ..message(id)
    }
}

fn thread_summary(root: Option<Message>) -> ThreadSummary {
    ThreadSummary {
        id: "t1".into(),
        root,
        title: "a thread".into(),
        reply_count: None,
        reply_count_exact: false,
        updated_at: "2026-09-25T12:00:00+00:00".into(),
        display_name: None,
        summary: None,
    }
}

fn client_config(
    scope: TokenScope,
    live_delivery: LiveDelivery,
    full: bool,
) -> ClientConfigResponse {
    ClientConfigResponse {
        chat_provider_name: "Chat".into(),
        channels: vec![
            channel(),
            ChannelInfo {
                alias: Some("ops".into()),
                added: true,
                writable: false,
                ..channel()
            },
            ChannelInfo {
                id: ChannelId("C0123ABCDE".into()),
                label: "team".into(),
                provider: Some("slack".into()),
                ..channel()
            },
        ],
        // `#199 removable-config-channels`. Present only in the full sample, as the server sends
        // it only to a write-scope caller with something hidden.
        hidden_channels: if full {
            vec![ChannelInfo {
                id: ChannelId("3333333333".into()),
                label: "home base".into(),
                ..channel()
            }]
        } else {
            Vec::new()
        },
        elevenlabs_agent_id: full.then(|| "agent".into()),
        conversational_voice: VoiceDescription {
            name: "voice".into(),
            settings_url: full.then(|| "https://voice.example/settings".into()),
            instance_id: full.then(|| "instance".into()),
        },
        read_aloud: Description {
            backend: "browser",
            label: "This device".into(),
            playback: if full {
                Playback::Audio
            } else {
                Playback::Browser
            },
            local_only: !full,
        },
        version: "0.0.0",
        replay_enabled: full,
        self_author_id: full.then(|| "9".into()),
        owner_author_id: full.then(|| "7".into()),
        live_poll_seconds: 5,
        live_delivery,
        channel_registration_supported: full,
        channel_discovery_supported: full,
        upstream_read_mark_supported: full,
        threading_supported: full,
        idempotent_posts_supported: full,
        speech_prep_enabled: full,
        providers: vec![
            ProviderDescription {
                key: "discord".into(),
                name: "Discord".into(),
                channel_registration_supported: false,
                channel_discovery_supported: false,
                upstream_read_mark_supported: false,
                threading_supported: full,
                idempotent_posts_supported: full,
                live_delivery,
                live_poll_seconds: 5,
                self_author_id: full.then(|| "9".into()),
                owner_author_id: full.then(|| "7".into()),
            },
            ProviderDescription {
                key: "slack".into(),
                name: "Slack".into(),
                channel_registration_supported: full,
                channel_discovery_supported: full,
                upstream_read_mark_supported: false,
                threading_supported: true,
                idempotent_posts_supported: false,
                live_delivery: LiveDelivery::Poll,
                live_poll_seconds: 30,
                self_author_id: full.then(|| "U0SELF0001".into()),
                owner_author_id: None,
            },
        ],
        summaries_unavailable: (!full).then(|| "elevenlabs.api_key is not configured".into()),
        token_scope: scope,
        noise_rules: full.then(|| NoiseRules {
            rules: vec!["Working…".into(), "Thinking*".into()],
            matching: "a message counts as read when its whole text is one of these",
        }),
    }
}

fn timeline(view: TimelineView, page: TimelinePage) -> TimelineResponse {
    TimelineResponse {
        channel: channel(),
        view,
        limit: 50,
        returned: page.messages.len() + page.threads.len(),
        page,
        dismissed: vec![MessageId("150".into())],
        reply_dismissals: None,
        pins_revision: None,
        untrusted_content_notice: "third-party text",
    }
}

/// A pin, with a thread or without. `#206 pin-message`.
fn pin(id: &str, threaded: bool) -> Pin {
    Pin {
        snapshot: PinSnapshot {
            message_id: MessageId(id.into()),
            author: "someone".into(),
            author_id: UserId("7".into()),
            author_is_bot: threaded,
            content: "hello".into(),
            truncated: threaded,
            timestamp: "2026-09-25T12:00:00+00:00".into(),
            thread_id: threaded.then(|| "t1".into()),
            thread_root: threaded,
        },
        pinned_at_ms: 1_790_000_000_000,
        // `#217 markdown-blocks`: present on the threaded sample and absent on the other.
        content_html: if threaded {
            "<p><strong>hello</strong></p>\n".into()
        } else {
            String::new()
        },
    }
}

fn to<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("a sample serializes")
}

fn samples() -> Value {
    let plain = message("300");
    let threaded = threaded_message("301");
    json!({
        "ApiErrorBody": [to(&ApiErrorBody { error: "bad_request", detail: "why".into() })],
        "ClientConfigResponse": [
            to(&client_config(TokenScope::Write, LiveDelivery::Push, true)),
            to(&client_config(TokenScope::Read, LiveDelivery::Poll, false)),
            to(&client_config(TokenScope::Read, LiveDelivery::Off, false)),
        ],
        "VoiceSession": [
            to(&VoiceSession {
                websocket_url: "wss://voice.example/session".into(),
                protocol: "vibe-talk-v1",
                provider: "Voice".into(),
                valid_for_seconds: Some(60),
                input_sample_rate: 16_000,
                output_sample_rate: 24_000,
            }),
            to(&VoiceSession {
                websocket_url: "wss://voice.example/session".into(),
                protocol: "vibe-talk-v1",
                provider: "Voice".into(),
                valid_for_seconds: None,
                input_sample_rate: 16_000,
                output_sample_rate: 24_000,
            }),
        ],
        "PendingPostResponse": [
            to(&PendingPostResponse {
                proposal: Some(PendingPost {
                    serial: 3,
                    handle: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                    channel_id: ChannelId("111".into()),
                    channel_name: "lead team".into(),
                    text: "on my way".into(),
                    reply_to: Some(MessageId("300".into())),
                    expires_in_ms: 120_000,
                }),
            }),
            to(&PendingPostResponse {
                proposal: Some(PendingPost {
                    serial: 4,
                    handle: "BBBBBBBBBBBBBBBBBBBBBB".into(),
                    channel_id: ChannelId("111".into()),
                    channel_name: "lead team".into(),
                    text: "on my way".into(),
                    reply_to: None,
                    expires_in_ms: 1,
                }),
            }),
            to(&PendingPostResponse { proposal: None }),
        ],
        "CommittedPostResponse": [to(&CommittedPostResponse {
            serial: 3,
            posted: plain.clone(),
            parts: vec![plain.clone()],
        })],
        "TimelineResponse": [
            to(&timeline(TimelineView::Main, TimelinePage {
                messages: vec![plain.clone(), threaded.clone()],
                has_threads: true,
                has_more: true,
                next_before: Some("cursor".into()),
                notice: Some("scope note".into()),
                ..TimelinePage::default()
            })),
            to(&timeline(TimelineView::Threads, TimelinePage {
                threads: vec![thread_summary(Some(plain.clone())), thread_summary(None)],
                ..TimelinePage::default()
            })),
            to(&timeline(TimelineView::Thread, TimelinePage {
                messages: vec![threaded.clone()],
                thread: Some(thread_summary(Some(plain.clone()))),
                ..TimelinePage::default()
            })),
            to(&timeline(TimelineView::Flat, TimelinePage::default())),
            // `#203 incremental-refresh`: a newest page that can be continued forward, a complete
            // delta with one upsert and one deletion, and an incomplete one that says there is
            // more and that the backend could not bring it up to date just now.
            // `#206 pin-message`: this one carries the pins' revision, as every read from a
            // server with a store does; the others leave it out, as a store-less server does.
            to(&TimelineResponse {
                pins_revision: Some(1_790_000_000_000),
                ..timeline(TimelineView::Main, TimelinePage {
                    messages: vec![plain.clone()],
                    next_after: Some("forward".into()),
                    ..TimelinePage::default()
                })
            }),
            // `#222 unread-replies`: Main's delta in a channel with threads says which of its
            // threads' replies are Done, from its oldest row up.
            to(&TimelineResponse {
                reply_dismissals: Some(ReplyDismissals {
                    from: MessageId("150".into()),
                    messages: vec![MessageId("299".into()), MessageId("150".into())],
                }),
                ..timeline(TimelineView::Main, TimelinePage {
                    messages: vec![threaded.clone()],
                    has_threads: true,
                    next_after: Some("forward-2".into()),
                    delta: Some(TimelineDelta {
                        more: false,
                        complete: true,
                        deleted: vec![MessageId("299".into())],
                        removed_threads: Vec::new(),
                    }),
                    ..TimelinePage::default()
                })
            }),
            to(&timeline(TimelineView::Threads, TimelinePage {
                threads: vec![thread_summary(None)],
                next_after: Some("forward-3".into()),
                delta: Some(TimelineDelta {
                    more: true,
                    complete: false,
                    deleted: Vec::new(),
                    removed_threads: vec!["gone-thread".into()],
                }),
                as_of: Some("2026-10-04T07:00:00Z".into()),
                ..TimelinePage::default()
            })),
        ],
        "LiveMessageEvent": [
            to(&LiveMessageEvent {
                message: &plain,
                replayed: false,
                from_tail: false,
                self_posted: false,
                untrusted_content_notice: "third-party text",
            }),
            to(&LiveMessageEvent {
                message: &threaded,
                replayed: true,
                from_tail: true,
                self_posted: true,
                untrusted_content_notice: "third-party text",
            }),
        ],
        "LiveDeleteEvent": [to(&LiveDeleteEvent {
            channel_id: ChannelId("100".into()),
            message_id: MessageId("300".into()),
            replayed: true,
            from_tail: false,
        })],
        "LiveResetEvent": [to(&LiveResetEvent { missed: 12, detail: "re-read" })],
        "VibeTalkV1ServerFrame": [
            to(&VibeTalkV1ServerFrame::SessionStarted { greeting: None, session_id: None }),
            to(&VibeTalkV1ServerFrame::SessionStarted {
                greeting: Some(true),
                session_id: Some("s1".into()),
            }),
            to(&VibeTalkV1ServerFrame::Transcript {
                role: TranscriptRole::User,
                text: "hi".into(),
                turn: None,
                is_final: None,
            }),
            to(&VibeTalkV1ServerFrame::Transcript {
                role: TranscriptRole::Assistant,
                text: "hello".into(),
                turn: Some(1),
                is_final: Some(false),
            }),
            to(&VibeTalkV1ServerFrame::TurnComplete { turn: None, interrupted: None }),
            to(&VibeTalkV1ServerFrame::TurnComplete { turn: Some(1), interrupted: Some(true) }),
            to(&VibeTalkV1ServerFrame::Error { message: None, detail: None }),
            to(&VibeTalkV1ServerFrame::Error {
                message: Some("failed".into()),
                detail: Some("why".into()),
            }),
        ],
        "Message": [
            to(&plain),
            to(&threaded),
            to(&Message {
                reactions: Some(Vec::new()),
                ..message("302")
            }),
        ],
        "ThreadSummary": [to(&thread_summary(Some(plain.clone()))), to(&thread_summary(None))],
        "PinsResponse": [
            to(&PinsResponse {
                channel: channel(),
                pins: vec![pin("300", false), pin("301", true)],
                revision: 1_790_000_000_000,
                limit: 100,
                pins_notice: "pins are this server's own",
                untrusted_content_notice: "third-party text",
            }),
            to(&PinsResponse {
                channel: channel(),
                pins: Vec::new(),
                revision: 0,
                limit: 100,
                pins_notice: "pins are this server's own",
                untrusted_content_notice: "third-party text",
            }),
        ],
        "PinChangeResponse": [
            to(&PinChangeResponse {
                channel: channel(),
                message_id: MessageId("301".into()),
                pinned: true,
                changed: true,
                pin: Some(pin("301", true)),
                unpinned: vec![MessageId("200".into())],
                revision: 1_790_000_000_001,
                pins_notice: "pins are this server's own",
            }),
            to(&PinChangeResponse {
                channel: channel(),
                message_id: MessageId("301".into()),
                pinned: false,
                changed: false,
                pin: None,
                unpinned: Vec::new(),
                revision: 1_790_000_000_001,
                pins_notice: "pins are this server's own",
            }),
        ],
        "VibeTalkV1ClientFrame": [
            to(&VibeTalkV1ClientFrame::AudioStart),
            to(&VibeTalkV1ClientFrame::AudioEnd),
            to(&VibeTalkV1ClientFrame::Prompt { text: "hi".into() }),
            to(&VibeTalkV1ClientFrame::Interrupt),
            to(&VibeTalkV1ClientFrame::Quit),
        ],
    })
}

fn root_names(schema: &Value) -> Vec<String> {
    schema["anyOf"]
        .as_array()
        .expect("the document lists its roots")
        .iter()
        .map(|root| {
            root["$ref"]
                .as_str()
                .and_then(|r| r.strip_prefix("#/$defs/"))
                .expect("every root is a named definition")
                .to_owned()
        })
        .collect()
}

#[test]
fn the_checked_in_schema_matches_the_rust_types() {
    check_or_update("vibe-talk.schema.json", &vibe_talk::contract::schema_text());
}

#[test]
fn the_checked_in_samples_match_what_the_rust_types_serialize() {
    let mut text = serde_json::to_string_pretty(&samples()).expect("samples serialize");
    text.push('\n');
    check_or_update("samples.json", &text);
}

#[test]
fn every_root_has_samples_and_every_sample_names_a_root() {
    let roots = root_names(&vibe_talk::contract::schema_document());
    let samples = samples();
    let sampled: Vec<&String> = samples.as_object().expect("an object").keys().collect();
    let mut expected: Vec<&String> = roots.iter().collect();
    expected.sort();
    assert_eq!(
        sampled, expected,
        "roots and samples must correspond one to one"
    );
}

#[test]
fn the_document_is_deterministic() {
    assert_eq!(
        vibe_talk::contract::schema_text(),
        vibe_talk::contract::schema_text()
    );
}

#[test]
fn server_frames_round_trip_and_an_unknown_optional_field_is_ignored() {
    let frame: VibeTalkV1ServerFrame = serde_json::from_value(json!({
        "type": "turn_complete",
        "turn": 3,
        "interrupted": false,
        "future_field": "ignored"
    }))
    .expect("a known frame with an extra field decodes");
    assert_eq!(
        frame,
        VibeTalkV1ServerFrame::TurnComplete {
            turn: Some(3),
            interrupted: Some(false)
        }
    );
    assert_eq!(
        serde_json::to_value(VibeTalkV1ClientFrame::Prompt { text: "x".into() }).unwrap(),
        json!({ "type": "prompt", "text": "x" })
    );
}

/// The generated browser validators accept every sample above and refuse broken ones. Like the
/// page suites, this FAILS when Node is missing rather than skipping: it needs no packages, only
/// the checked-in `web/contract.js`.
#[test]
fn the_browser_validators_accept_the_samples_and_refuse_broken_values() {
    const SUITE: &str = "tests/js/contract.test.mjs";
    let output = std::process::Command::new("node")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg(SUITE)
        .output()
        .unwrap_or_else(|error| panic!("could not run node, which {SUITE} needs: {error}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{SUITE} failed:\n{text}");
    assert!(
        text.contains("# fail 0"),
        "{SUITE} reported failures:\n{text}"
    );
    assert!(
        !text.contains("# pass 0\n"),
        "{SUITE} collected no tests:\n{text}"
    );
}
