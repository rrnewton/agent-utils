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
    ApiErrorBody, BookmarkResponse, ClientConfigResponse, CommittedPostResponse, LiveDeleteEvent,
    LiveDelivery, LiveMessageEvent, LiveResetEvent, MessageCacheRequest, MessageCacheStatus,
    MessagesResponse, NoiseRules, PendingPost, PendingPostResponse, PinChangeResponse,
    PinsResponse, ProposalSettled, ProviderDescription, ReplayUnavailable, ReplyDismissals,
    TimelineResponse, TokenScope, TranscriptRole, UpstreamClearAttemptView,
    UpstreamClearNowOutcome, UpstreamClearNowResponse, UpstreamClearState, VibeTalkV1ClientFrame,
    VibeTalkV1ServerFrame, VoicePostPrefix, VoicePromptSettings, VoiceSend, VoiceSettings,
};
use vibe_talk::contract::{PromptDelivery, PromptTransport, SessionPrompt};
use vibe_talk::conversation::{VoiceDescription, VoiceSession};
use vibe_talk::model::{ChannelId, ChannelInfo, Heading, Message, MessageId, Reaction, UserId};
use vibe_talk::post_gate::ProposalFate;
use vibe_talk::speech::{Description, Playback};
use vibe_talk::store::{
    Bookmark, BookmarkAnchor, BookmarkPlace, Pin, PinSnapshot, SendHow, SendOrigin, SendOutcome,
    SentRecord, UpstreamClearMode, UpstreamClearResult,
};
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
        write_source: None,
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
        edited_timestamp: None,
        reply_to: None,
        content: "hello".into(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
        heading: None,
        mentions: Vec::new(),
        upstream_url: None,
    }
}

fn threaded_message(id: &str) -> Message {
    Message {
        thread: Some(MessageThread {
            hue: None,
            id: "t1".into(),
            root_message_id: Some(MessageId("200".into())),
            is_root: false,
            reply_count: Some(3),
            reply_count_exact: true,
        }),
        spoken_time: "noon".into(),
        // `#320 slack-edited-mark`: edited here, not on the plain sample, so the generated
        // validators are proven against both.
        edited_timestamp: Some("2026-09-25T12:04:30.000100Z".into()),
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
                // `#312 slack-reactions`: the owner's own reaction, as Slack reports one.
                owner_reacted: true,
            },
            Reaction {
                emoji: "party-parrot".into(),
                custom: true,
                custom_id: Some("112233".into()),
                count: 2,
                owner_reacted: false,
            },
        ]),
        // `#242 rich-headers`: every piece here, none on the plain sample, and a tag alone on the
        // threaded pin.
        content: "[build-bot] Status, 15:05Z. hello".into(),
        heading: Some(Heading {
            tag: Some("build-bot".into()),
            time: Some("15:05Z".into()),
            title: Some("Status".into()),
            cut: 28,
        }),
        // `#408 copy-upstream-link`: present here, absent on the plain sample.
        upstream_url: Some("https://chat.google.com/room/AAAa1/BBBb2/CCCc3".into()),
        ..message(id)
    }
}

fn thread_summary(root: Option<Message>) -> ThreadSummary {
    ThreadSummary {
        hue: None,
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
        // `#343 pwa-app-name`. Present in the full sample, absent in the other, as from an older
        // server.
        app_name: full.then(|| "vibe-talk at work".into()),
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
        // `#270 voice-transcript-markers`. The plain sample is a websocket deployment with replay
        // turned on whose bridge has not said it takes a recap (`#331 resume-on-websocket`).
        replay_unavailable: (!full).then_some(ReplayUnavailable::BridgeUnconfirmed),
        transcript_records_supported: full,
        self_author_id: full.then(|| "9".into()),
        owner_author_id: full.then(|| "7".into()),
        // `#323 slack-system-bots`: both present in the full sample, both absent in the other.
        coding_agent_ids: if full {
            vec!["1000000000000000001".into()]
        } else {
            Vec::new()
        },
        coding_agents_named_only: full,
        live_poll_seconds: 5,
        live_delivery,
        channel_registration_supported: full,
        channel_discovery_supported: full,
        upstream_read_mark_supported: full,
        upstream_clear_supported: full,
        threading_supported: full,
        replies_in_thread: full,
        idempotent_posts_supported: full,
        speech_prep_enabled: full,
        providers: vec![
            ProviderDescription {
                key: "discord".into(),
                name: "Discord".into(),
                channel_registration_supported: false,
                channel_discovery_supported: false,
                upstream_read_mark_supported: false,
                upstream_clear_supported: full,
                threading_supported: full,
                replies_in_thread: false,
                idempotent_posts_supported: full,
                live_delivery,
                live_poll_seconds: 5,
                self_author_id: full.then(|| "9".into()),
                owner_author_id: full.then(|| "7".into()),
                coding_agent_ids: if full {
                    vec!["1000000000000000001".into()]
                } else {
                    Vec::new()
                },
                coding_agents_named_only: full,
            },
            ProviderDescription {
                key: "slack".into(),
                name: "Slack".into(),
                channel_registration_supported: full,
                channel_discovery_supported: full,
                upstream_read_mark_supported: false,
                upstream_clear_supported: false,
                threading_supported: true,
                replies_in_thread: true,
                idempotent_posts_supported: false,
                live_delivery: LiveDelivery::Poll,
                live_poll_seconds: 30,
                self_author_id: full.then(|| "U0SELF0001".into()),
                owner_author_id: None,
                // A Slack provider guesses no coding agent, named or not.
                coding_agent_ids: Vec::new(),
                coding_agents_named_only: true,
            },
        ],
        summaries_unavailable: (!full).then(|| "elevenlabs.api_key is not configured".into()),
        token_scope: scope,
        noise_rules: full.then(|| NoiseRules {
            rules: vec!["Working…".into(), "Thinking*".into()],
            matching: "a message counts as read when its whole text is one of these",
        }),
        voice_settings: full.then_some(VoiceSettings {
            ask_before_send: true,
            changeable: true,
        }),
        voice_prompt: full.then(|| voice_prompt(true, PromptDelivery::Page)),
        voice_post_prefix: full.then(|| post_prefix(None, true)),
        // `#208 persistent-cache`. Present only in the full sample, as the server sends it only to
        // a write-scope caller of a server with a `[message_cache]` table.
        message_cache: full.then(|| message_cache_status(true)),
    }
}

/// The tag on the voice agent's posts: the shipped default, the owner's own, or his empty one.
/// `#303 voice-post-prefix`.
fn post_prefix(saved: Option<&str>, changeable: bool) -> VoicePostPrefix {
    const DEFAULT: &str = "(Voice-agent relayed, from user) ";
    VoicePostPrefix {
        prefix: saved.unwrap_or(DEFAULT).into(),
        default_prefix: DEFAULT.into(),
        custom: saved.is_some(),
        max_chars: 100,
        changeable,
    }
}

/// The message cache's status, open and holding something, or switched off. `#208
/// persistent-cache`.
fn message_cache_status(open: bool) -> MessageCacheStatus {
    MessageCacheStatus {
        available: open,
        enabled: open,
        reason: (!open).then_some("switched_off"),
        bytes: if open { 1_048_576 } else { 0 },
        messages: if open { 1_200 } else { 0 },
        channels: if open { 3 } else { 0 },
        oldest_covered: open.then(|| "2026-10-01T00:00:00Z".into()),
        max_days: 14,
        max_megabytes: 256,
    }
}

/// The owner's words for the voice agent, with an edited base or the shipped one.
/// `#272 voice-agent-instructions`.
fn voice_prompt(edited: bool, delivery: PromptDelivery) -> VoicePromptSettings {
    VoicePromptSettings {
        instructions: if edited { "Call me Sam." } else { "" }.into(),
        system_prompt: if edited {
            "You read me the chat."
        } else {
            "You are a voice assistant."
        }
        .into(),
        system_prompt_edited: edited,
        edited_from_version: edited.then_some(8),
        prompt_version: 9,
        max_instructions_chars: 4_000,
        max_system_prompt_chars: 20_000,
        changeable: edited,
        delivery,
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
        bookmark: None,
        untrusted_content_notice: "third-party text",
    }
}

/// A bookmark above a message in a thread, below one that is not, or none at all.
/// `#240 reading-bookmark`.
fn bookmark(place: Option<BookmarkPlace>) -> Bookmark {
    Bookmark {
        revision: if place.is_some() {
            1_790_000_000_002
        } else {
            0
        },
        anchor: place.map(|place| BookmarkAnchor {
            message_id: MessageId("301".into()),
            place,
            timestamp: "2026-09-25T12:00:00+00:00".into(),
            thread_id: (place == BookmarkPlace::Above).then(|| "t1".into()),
        }),
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
            content: if threaded { "(ops2) hello" } else { "hello" }.into(),
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
        heading: threaded.then(|| Heading {
            tag: Some("ops2".into()),
            time: None,
            title: None,
            cut: 7,
        }),
        // `#408 copy-upstream-link`: present on the threaded sample and absent on the other.
        upstream_url: threaded
            .then(|| "https://acme.slack.com/archives/C0123ABCD/p1700000000000100".into()),
    }
}

/// One send as the post gate's send memory files it. `#270 voice-transcript-markers`.
fn sent_record(outcome: SendOutcome, how: SendHow) -> SentRecord {
    SentRecord {
        serial: 5,
        send_seq: 3,
        channel_id: "111".into(),
        channel_name: "lead team".into(),
        reply_to: None,
        reply_to_author: None,
        reply_to_excerpt: None,
        thread_id: None,
        thread_name: None,
        message_id: Some("900".into()),
        parts_posted: 1,
        parts_total: 1,
        uncertain_part: None,
        unsent_chars: 0,
        outcome,
        how,
        origin: SendOrigin::Commit,
        text_truncated: false,
        text_chars: 9,
        begun_at_ms: 1_699_999_999_000,
        sent_at_ms: 1_699_999_999_400,
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
                agent_prompt: None,
            }),
            to(&VoiceSession {
                websocket_url: "wss://voice.example/session".into(),
                protocol: "vibe-talk-v1",
                provider: "Voice".into(),
                valid_for_seconds: None,
                input_sample_rate: 16_000,
                output_sample_rate: 24_000,
                agent_prompt: None,
            }),
            // `#272 voice-agent-instructions`: a hosted agent handed the owner's words each of the
            // three ways, with what its configuration said present and absent.
            to(&VoiceSession {
                websocket_url: "wss://voice.example/session".into(),
                protocol: "elevenlabs",
                provider: "ElevenLabs".into(),
                valid_for_seconds: Some(900),
                input_sample_rate: 16_000,
                output_sample_rate: 16_000,
                agent_prompt: Some(SessionPrompt {
                    transport: PromptTransport::PromptOverride,
                    text: "You read me the chat.".into(),
                    system_prompt_applied: true,
                    instructions_applied: true,
                    override_allowed: Some(true),
                }),
            }),
            to(&VoiceSession {
                websocket_url: "wss://voice.example/session".into(),
                protocol: "elevenlabs",
                provider: "ElevenLabs".into(),
                valid_for_seconds: Some(900),
                input_sample_rate: 16_000,
                output_sample_rate: 16_000,
                agent_prompt: Some(SessionPrompt {
                    transport: PromptTransport::ContextualUpdate,
                    text: "Call me Sam.".into(),
                    system_prompt_applied: false,
                    instructions_applied: true,
                    override_allowed: None,
                }),
            }),
            to(&VoiceSession {
                websocket_url: "wss://voice.example/session".into(),
                protocol: "elevenlabs",
                provider: "ElevenLabs".into(),
                valid_for_seconds: Some(900),
                input_sample_rate: 16_000,
                output_sample_rate: 16_000,
                agent_prompt: Some(SessionPrompt {
                    transport: PromptTransport::NotSent,
                    text: String::new(),
                    system_prompt_applied: false,
                    instructions_applied: false,
                    override_allowed: Some(false),
                }),
            }),
        ],
        "PendingPostResponse": [
            to(&PendingPostResponse {
                proposal: Some(PendingPost {
                    serial: 3,
                    instance: "cnVuLW9uZQ".into(),
                    handle: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                    channel_id: ChannelId("111".into()),
                    channel_name: "lead team".into(),
                    // `#303 voice-post-prefix`: the owner's tag on the agent's words, and which
                    // part of the text it is.
                    text: "(Voice-agent relayed, from user) on my way".into(),
                    prefix: "(Voice-agent relayed, from user) ".into(),
                    reply_to: Some(MessageId("300".into())),
                    reply_to_author: Some("Alex".into()),
                    reply_to_excerpt: Some("is the build green yet…".into()),
                    thread_name: Some("release".into()),
                    expires_in_ms: 120_000,
                }),
                instance: "cnVuLW9uZQ".into(),
                sends: Vec::new(),
                sends_through: Some(0),
                settled: Vec::new(),
                now_ms: Some(1_700_000_000_000),
            }),
            to(&PendingPostResponse {
                proposal: Some(PendingPost {
                    serial: 4,
                    instance: "cnVuLW9uZQ".into(),
                    handle: "BBBBBBBBBBBBBBBBBBBBBB".into(),
                    channel_id: ChannelId("111".into()),
                    channel_name: "lead team".into(),
                    text: "on my way".into(),
                    prefix: String::new(),
                    reply_to: None,
                    reply_to_author: None,
                    reply_to_excerpt: None,
                    thread_name: None,
                    expires_in_ms: 1,
                }),
                instance: "cnVuLW9uZQ".into(),
                sends: Vec::new(),
                sends_through: Some(2),
                settled: vec![ProposalSettled {
                    serial: 3,
                    fate: ProposalFate::Superseded,
                }],
                now_ms: Some(1_700_000_000_000),
            }),
            to(&PendingPostResponse {
                proposal: None,
                instance: "cnVuLXR3bw".into(),
                sends: vec![
                    VoiceSend {
                        text: "on my way".into(),
                        detail: sent_record(SendOutcome::Sent, SendHow::Tap),
                        instance: "cnVuLXR3bw".into(),
                    },
                    VoiceSend {
                        text: "the first part".into(),
                        detail: SentRecord {
                            serial: 6,
                            send_seq: 4,
                            reply_to: Some("300".into()),
                            reply_to_author: Some("Alex".into()),
                            reply_to_excerpt: Some("is the build green yet…".into()),
                            thread_id: Some("t1".into()),
                            thread_name: Some("release".into()),
                            parts_posted: 1,
                            parts_total: 3,
                            uncertain_part: Some(2),
                            unsent_chars: 1_900,
                            text_truncated: true,
                            ..sent_record(SendOutcome::Partial, SendHow::Voice)
                        },
                        instance: "cnVuLXR3bw".into(),
                    },
                    VoiceSend {
                        text: "rebase onto main".into(),
                        detail: SentRecord {
                            serial: 7,
                            send_seq: 5,
                            message_id: None,
                            parts_posted: 0,
                            origin: SendOrigin::Rest,
                            ..sent_record(SendOutcome::Unconfirmed, SendHow::Direct)
                        },
                        instance: "cnVuLXR3bw".into(),
                    },
                ],
                sends_through: Some(5),
                settled: [
                    ProposalFate::Committed,
                    ProposalFate::Cancelled,
                    ProposalFate::Expired,
                    ProposalFate::Superseded,
                    ProposalFate::Refused,
                ]
                .into_iter()
                .zip(1..)
                .map(|(fate, serial)| ProposalSettled { serial, fate })
                .collect(),
                now_ms: Some(1_700_000_000_000),
            }),
            // An older server's answer, from before `#270 voice-transcript-markers`: the proposal and
            // nothing else. A page left open across a rollback to it must still read the proposal.
            json!({
                "proposal": {
                    "serial": 2,
                    "handle": "CCCCCCCCCCCCCCCCCCCCCC",
                    "channel_id": "111",
                    "channel_name": "lead team",
                    "text": "on my way",
                    "reply_to": null,
                    "expires_in_ms": 5000
                }
            }),
        ],
        "CommittedPostResponse": [
            to(&CommittedPostResponse {
                serial: 3,
                send_seq: Some(3),
                posted: plain.clone(),
                parts: vec![plain.clone()],
            }),
            to(&CommittedPostResponse {
                serial: 3,
                send_seq: None,
                posted: plain.clone(),
                parts: vec![plain.clone()],
            }),
        ],
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
            // `#240 reading-bookmark`: and the bookmark, as every read from a server with a store
            // does: here one is set, on the next none is, and the rest leave the field out.
            to(&TimelineResponse {
                pins_revision: Some(1_790_000_000_000),
                bookmark: Some(bookmark(Some(BookmarkPlace::Above))),
                ..timeline(TimelineView::Main, TimelinePage {
                    messages: vec![plain.clone()],
                    next_after: Some("forward".into()),
                    ..TimelinePage::default()
                })
            }),
            // `#222 unread-replies`: Main's delta in a channel with threads says which of its
            // threads' replies are Done, from its oldest row up.
            to(&TimelineResponse {
                bookmark: Some(bookmark(None)),
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
            to(&VibeTalkV1ServerFrame::SessionStarted {
                greeting: None,
                session_id: None,
                capabilities: Vec::new(),
            }),
            to(&VibeTalkV1ServerFrame::SessionStarted {
                greeting: Some(true),
                session_id: Some("s1".into()),
                // `#331 resume-on-websocket`: the bridge's half of the hello.
                capabilities: vec!["replay".into()],
            }),
            to(&VibeTalkV1ServerFrame::ReplayAck { accepted: true, reason: None }),
            to(&VibeTalkV1ServerFrame::ReplayAck {
                accepted: false,
                reason: Some("too long for this model".into()),
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
                heading: None,
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
        // `#232 upstream-mark-read`: a channel never set, one that pushed and is failing now, and
        // one its provider refused, so every optional field is seen present and absent.
        "UpstreamClearState": [
            to(&UpstreamClearState {
                channel: channel(),
                provider_name: "Google Chat".into(),
                supported: true,
                mode: UpstreamClearMode::Off,
                enabled_at_ms: None,
                pushed: None,
                previous_cursor: None,
                last: None,
                failures: 0,
                pending: false,
                retry_at_ms: None,
                unavailable: None,
            }),
            to(&UpstreamClearState {
                channel: channel(),
                provider_name: "Google Chat".into(),
                supported: true,
                mode: UpstreamClearMode::On,
                enabled_at_ms: Some(1_790_000_000_000),
                pushed: Some(UpstreamClearAttemptView {
                    at_ms: 1_790_000_010_000,
                    message_id: MessageId("300".into()),
                    message_at_ms: 1_790_000_005_000,
                    result: UpstreamClearResult::Advanced,
                    target: Some("2026-09-25T12:00:00Z".into()),
                    previous: Some("2026-09-25T11:00:00Z".into()),
                    error: None,
                }),
                previous_cursor: Some("2026-09-25T11:00:00Z".into()),
                last: Some(UpstreamClearAttemptView {
                    at_ms: 1_790_000_060_000,
                    message_id: MessageId("301".into()),
                    message_at_ms: 1_790_000_050_000,
                    result: UpstreamClearResult::Failed,
                    target: None,
                    previous: None,
                    error: Some("Google Chat returned HTTP 502: mark-read failed".into()),
                }),
                failures: 1,
                pending: true,
                retry_at_ms: Some(1_790_000_090_000),
                unavailable: None,
            }),
            to(&UpstreamClearState {
                channel: channel(),
                provider_name: "Google Chat".into(),
                supported: true,
                mode: UpstreamClearMode::DryRun,
                enabled_at_ms: Some(1_790_000_000_000),
                pushed: None,
                previous_cursor: None,
                last: Some(UpstreamClearAttemptView {
                    at_ms: 1_790_000_010_000,
                    message_id: MessageId("300".into()),
                    message_at_ms: 1_790_000_005_000,
                    result: UpstreamClearResult::Unavailable,
                    target: None,
                    previous: None,
                    error: Some("one thread of a larger space".into()),
                }),
                failures: 0,
                pending: false,
                retry_at_ms: None,
                unavailable: Some("one thread of a larger space".into()),
            }),
        ],
        // `#240 reading-bookmark`: set above a message in a thread, set below one that is not,
        // and cleared, so `anchor` is seen as an object, with and without `thread_id`, and null.
        "BookmarkResponse": [
            to(&BookmarkResponse {
                channel: channel(),
                bookmark: bookmark(Some(BookmarkPlace::Above)),
                changed: true,
                bookmark_notice: "the bookmark is this server's own",
            }),
            to(&BookmarkResponse {
                channel: channel(),
                bookmark: bookmark(Some(BookmarkPlace::Below)),
                changed: false,
                bookmark_notice: "the bookmark is this server's own",
            }),
            to(&BookmarkResponse {
                channel: channel(),
                bookmark: bookmark(None),
                changed: true,
                bookmark_notice: "the bookmark is this server's own",
            }),
        ],
        // `#253 upstream-clear-polish`: a press that pushed, one with nothing newer to push, and
        // one whose push failed.
        "UpstreamClearNowResponse": [
            to(&UpstreamClearNowResponse {
                outcome: UpstreamClearNowOutcome::Pushed,
                message_id: Some(MessageId("300".into())),
                message_at_ms: Some(1_790_000_005_000),
                attempt: Some(UpstreamClearAttemptView {
                    at_ms: 1_790_000_010_000,
                    message_id: MessageId("300".into()),
                    message_at_ms: 1_790_000_005_000,
                    result: UpstreamClearResult::Advanced,
                    target: Some("2026-09-25T12:00:00Z".into()),
                    previous: Some("2026-09-25T11:00:00Z".into()),
                    error: None,
                }),
                state: UpstreamClearState {
                    channel: channel(),
                    provider_name: "Google Chat".into(),
                    supported: true,
                    mode: UpstreamClearMode::On,
                    enabled_at_ms: Some(1_790_000_000_000),
                    pushed: Some(UpstreamClearAttemptView {
                        at_ms: 1_790_000_010_000,
                        message_id: MessageId("300".into()),
                        message_at_ms: 1_790_000_005_000,
                        result: UpstreamClearResult::Advanced,
                        target: Some("2026-09-25T12:00:00Z".into()),
                        previous: Some("2026-09-25T11:00:00Z".into()),
                        error: None,
                    }),
                    previous_cursor: Some("2026-09-25T11:00:00Z".into()),
                    last: Some(UpstreamClearAttemptView {
                        at_ms: 1_790_000_010_000,
                        message_id: MessageId("300".into()),
                        message_at_ms: 1_790_000_005_000,
                        result: UpstreamClearResult::Advanced,
                        target: Some("2026-09-25T12:00:00Z".into()),
                        previous: Some("2026-09-25T11:00:00Z".into()),
                        error: None,
                    }),
                    failures: 0,
                    pending: false,
                    retry_at_ms: None,
                    unavailable: None,
                },
            }),
            to(&UpstreamClearNowResponse {
                outcome: UpstreamClearNowOutcome::Covered,
                message_id: Some(MessageId("300".into())),
                message_at_ms: Some(1_790_000_005_000),
                attempt: None,
                state: UpstreamClearState {
                    channel: channel(),
                    provider_name: "Google Chat".into(),
                    supported: true,
                    mode: UpstreamClearMode::DryRun,
                    enabled_at_ms: Some(1_790_000_000_000),
                    pushed: None,
                    previous_cursor: None,
                    last: None,
                    failures: 0,
                    pending: false,
                    retry_at_ms: None,
                    unavailable: None,
                },
            }),
            to(&UpstreamClearNowResponse {
                outcome: UpstreamClearNowOutcome::Failed,
                message_id: Some(MessageId("300".into())),
                message_at_ms: Some(1_790_000_005_000),
                attempt: Some(UpstreamClearAttemptView {
                    at_ms: 1_790_000_010_000,
                    message_id: MessageId("300".into()),
                    message_at_ms: 1_790_000_005_000,
                    result: UpstreamClearResult::Failed,
                    target: None,
                    previous: None,
                    error: Some("Google Chat returned HTTP 502: mark-read failed".into()),
                }),
                state: UpstreamClearState {
                    channel: channel(),
                    provider_name: "Google Chat".into(),
                    supported: true,
                    mode: UpstreamClearMode::On,
                    enabled_at_ms: Some(1_790_000_000_000),
                    pushed: None,
                    previous_cursor: None,
                    last: Some(UpstreamClearAttemptView {
                        at_ms: 1_790_000_010_000,
                        message_id: MessageId("300".into()),
                        message_at_ms: 1_790_000_005_000,
                        result: UpstreamClearResult::Failed,
                        target: None,
                        previous: None,
                        error: Some("Google Chat returned HTTP 502: mark-read failed".into()),
                    }),
                    failures: 1,
                    pending: true,
                    retry_at_ms: Some(1_790_000_040_000),
                    unavailable: None,
                },
            }),
        ],
        // `#264 voice-send-approval`: asking and saved, sending at once, and a server that keeps
        // no settings and so always asks.
        "VoiceSettings": [
            to(&VoiceSettings { ask_before_send: true, changeable: true }),
            to(&VoiceSettings { ask_before_send: false, changeable: true }),
            to(&VoiceSettings { ask_before_send: true, changeable: false }),
        ],
        // `#272 voice-agent-instructions`: an owner's edited copy for a hosted agent, and the
        // shipped prompt on a bridge with no store.
        "VoicePromptSettings": [
            to(&voice_prompt(true, PromptDelivery::Page)),
            to(&voice_prompt(false, PromptDelivery::Profile)),
        ],
        // `#303 voice-post-prefix`: the shipped tag, the owner's own, his empty one that turns the
        // tag off, and a server that keeps no settings.
        "VoicePostPrefix": [
            to(&post_prefix(None, true)),
            to(&post_prefix(Some("[voice] "), true)),
            to(&post_prefix(Some(""), true)),
            to(&post_prefix(None, false)),
        ],
        // `#208 persistent-cache`: a current window, and one a provider served from a copy it
        // could not bring up to date, so `as_of` is seen omitted and present.
        "MessagesResponse": [
            to(&MessagesResponse {
                channel: channel(),
                messages: vec![plain.clone(), threaded.clone()],
                complete: false,
                dismissed: vec![MessageId("300".into())],
                as_of: None,
                untrusted_content_notice: "third-party text",
            }),
            to(&MessagesResponse {
                channel: channel(),
                messages: vec![plain.clone()],
                complete: true,
                dismissed: Vec::new(),
                as_of: Some("2026-10-12T09:00:00Z".into()),
                untrusted_content_notice: "third-party text",
            }),
        ],
        // `#208 persistent-cache`: an open cache with coverage, and one switched off, so `reason`
        // and `oldest_covered` are each seen present and omitted.
        "MessageCacheStatus": [
            to(&message_cache_status(true)),
            to(&message_cache_status(false)),
        ],
        "MessageCacheRequest": [
            to(&MessageCacheRequest { enabled: false }),
            to(&MessageCacheRequest { enabled: true }),
        ],
        "VibeTalkV1ClientFrame": [
            to(&VibeTalkV1ClientFrame::AudioStart),
            to(&VibeTalkV1ClientFrame::AudioEnd),
            to(&VibeTalkV1ClientFrame::Prompt { text: "hi".into() }),
            to(&VibeTalkV1ClientFrame::Interrupt),
            to(&VibeTalkV1ClientFrame::Quit),
            to(&VibeTalkV1ClientFrame::Replay {
                text: "You are resuming an earlier voice conversation.".into(),
                lines: 2,
                dropped: 0,
                words: 9,
            }),
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
