//! The voice agent's operating instructions and what it can actually do, in one provider-neutral
//! shape.
//!
//! `#14 voice-agent-prompt`. A hosted agent is configured in its vendor's console and a
//! deployment-managed agent is configured by whatever opens its session. Two hand-maintained
//! copies of the same instructions drift the first time one of them is edited, and the one that
//! drifts is the one nobody is looking at. So the prompt lives in checked-in files — the
//! scope-neutral [`SYSTEM_PROMPT`] and exactly one of [`SEND_SECTION`] or [`READ_ONLY_SECTION`] —
//! and everything else — the operator pasting it into a console, a private bridge fetching it at
//! session open — reads those files.
//!
//! **The prompt follows the credential.** A model told "send a chat message when the user asks"
//! while holding a read token will promise a send it cannot make, then invent a reason when the
//! call is refused. So [`prompt`] composes the base with the send rules only when a write tool is
//! actually available, and with a plain "you can read but cannot send" otherwise.
//!
//! **Versioned, and the version is enforced.** [`PROMPT_VERSION`] is what a consumer logs to
//! prove which instructions a live call received. A test pins each variant's [`fingerprint`] to
//! that version, so editing the prompt without bumping the version fails the build rather than
//! shipping two different prompts under one name.
//!
//! **Capability data rather than capability prose.** The prompt tells the agent to describe its
//! tools accurately and to use the exact mention token the chat tools return. Both depend on the
//! deployment — which tools this credential may call, which chat service is behind them — so they
//! are delivered as data beside the prompt ([`VoiceAgentProfile`]), never baked into its wording.
//! The prompt stays free of any provider's name or syntax.
//!
//! **Startup timing is content-free.** [`StartupTiming`] is a fixed set of named phases, each an
//! integer millisecond offset. Unknown fields are refused rather than ignored, so the one line
//! it writes to the log cannot become a channel for transcript text.

use serde::{Deserialize, Serialize};

use crate::auth::Scope;
use crate::mcp::{ApprovalMode, ToolDescriptor};

/// The scope-neutral operating instructions every conversational provider is given.
///
/// Never used alone: [`prompt`] always appends the section that matches the caller's scope.
pub const SYSTEM_PROMPT: &str = include_str!("../prompts/voice-agent-system.txt");

/// Appended when this caller may send: when and how to send, and how to mention a person.
pub const SEND_SECTION: &str = include_str!("../prompts/voice-agent-send.txt");

/// Appended when this caller may only read: it cannot send, and says so when asked.
pub const READ_ONLY_SECTION: &str = include_str!("../prompts/voice-agent-read-only.txt");

/// Stable name of the prompt, for logs and for consumers that hold more than one.
pub const PROMPT_ID: &str = "voice-agent-system";

/// Bump this whenever any of the three prompt files changes, and update
/// [`PINNED_READ_FINGERPRINT`] and [`PINNED_WRITE_FINGERPRINT`] to match. The test that compares
/// them is what makes the version mean something.
pub const PROMPT_VERSION: u32 = 2;

/// The [`fingerprint`] of the read-only prompt at [`PROMPT_VERSION`].
pub const PINNED_READ_FINGERPRINT: &str = "fnv1a64:fe95ee12cbcd38c4";

/// The [`fingerprint`] of the sending prompt at [`PROMPT_VERSION`].
pub const PINNED_WRITE_FINGERPRINT: &str = "fnv1a64:c97cb6194902ccba";

/// A content fingerprint a consumer can log without logging the prompt.
///
/// FNV-1a over the UTF-8 bytes. It is an identity check between two copies of a file this
/// repository publishes, not a security boundary, so a cryptographic hash would add a dependency
/// and prove nothing more.
#[must_use]
pub fn fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// The prompt, named and versioned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptDescriptor {
    /// Stable prompt name.
    pub id: &'static str,
    /// Monotonic version; see [`PROMPT_VERSION`].
    pub version: u32,
    /// `read` or `write`: which section follows the base. Matches the profile's `write_available`.
    pub variant: &'static str,
    /// Content fingerprint of `text`; see [`fingerprint`].
    pub fingerprint: String,
    /// The full instructions, to be used verbatim as the agent's system prompt.
    pub text: String,
}

/// The prompt this build ships for a caller that can, or cannot, send.
#[must_use]
pub fn prompt(write_available: bool) -> PromptDescriptor {
    let (variant, section) = if write_available {
        ("write", SEND_SECTION)
    } else {
        ("read", READ_ONLY_SECTION)
    };
    let text = format!("{SYSTEM_PROMPT}\n{section}");
    PromptDescriptor {
        id: PROMPT_ID,
        version: PROMPT_VERSION,
        variant,
        fingerprint: fingerprint(&text),
        text,
    }
}

/// Whether a tool only reads, or changes something outside this server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAccess {
    /// Reads chat history; never changes anything.
    Read,
    /// Sends a message, or otherwise acts outside this server.
    Write,
}

/// One tool, as the agent should understand it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCapability {
    /// Tool name exactly as the MCP endpoint lists it.
    pub name: &'static str,
    /// Read or write.
    pub access: ToolAccess,
    /// Whether the agent must obtain the user's spoken confirmation before calling it.
    pub requires_confirmation: bool,
    /// Whether THIS caller's MCP session will list and accept it.
    ///
    /// A write tool is listed here as unavailable to a read-scope caller rather than omitted, so
    /// an agent asked "can you send a message?" can answer truthfully instead of guessing.
    pub available: bool,
}

/// How the agent should mention a person in a sent message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MentionCapability {
    /// Always `returned_token`: copy the token the read tools print beside an author's name.
    ///
    /// Stated as data because the syntax differs between chat services and the prompt must not
    /// guess it. There is deliberately no lookup tool for people who have not posted.
    pub source: &'static str,
}

/// Everything a conversational provider needs to configure one call.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VoiceAgentProfile {
    /// The operating instructions, matching `write_available`.
    pub prompt: PromptDescriptor,
    /// Human-readable name of the chat service behind the tools, for the agent to say aloud.
    pub chat_provider_name: String,
    /// IANA zone the tools already render local times in.
    pub time_zone: String,
    /// Path of the Streamable HTTP MCP endpoint on this server.
    pub mcp_path: &'static str,
    /// The scope of the credential that asked. The MCP endpoint applies the same scope.
    pub scope: &'static str,
    /// Every tool the MCP endpoint can offer, and whether this caller may use it.
    pub tools: Vec<ToolCapability>,
    /// Whether any write tool is available to this caller.
    pub write_available: bool,
    /// How to mention a person.
    pub mentions: MentionCapability,
}

/// Build the profile for a caller holding `scope`.
///
/// Derived from the same manifest the MCP endpoint lists tools from, with the same two filters —
/// `mcp_exposed`, and write tools only at write scope — so this answer and `tools/list` cannot
/// disagree about what the agent may call.
#[must_use]
pub fn profile(
    manifest: &[ToolDescriptor],
    scope: Scope,
    chat_provider_name: &str,
    time_zone: &str,
) -> VoiceAgentProfile {
    let tools: Vec<ToolCapability> = manifest
        .iter()
        .filter(|tool| tool.mcp_exposed)
        .map(|tool| ToolCapability {
            name: tool.name,
            access: if tool.mutates {
                ToolAccess::Write
            } else {
                ToolAccess::Read
            },
            requires_confirmation: tool.approval == ApprovalMode::RequiresApproval,
            available: !tool.mutates || scope >= Scope::Write,
        })
        .collect();
    let write_available = tools
        .iter()
        .any(|tool| tool.access == ToolAccess::Write && tool.available);
    VoiceAgentProfile {
        prompt: prompt(write_available),
        chat_provider_name: chat_provider_name.to_owned(),
        time_zone: time_zone.to_owned(),
        mcp_path: crate::mcp::transport::MCP_PATH,
        scope: match scope {
            Scope::Read => "read",
            Scope::Write => "write",
        },
        tools,
        write_available,
        mentions: MentionCapability {
            source: "returned_token",
        },
    }
}

/// The longest phase offset accepted, in milliseconds. A startup slower than ten minutes is not a
/// measurement anybody will act on, and the bound keeps a malformed client from logging nonsense.
pub const MAX_PHASE_MS: u32 = 600_000;

/// Wire protocols a timing record may name.
const PROTOCOLS: &[&str] = &["elevenlabs", "vibe-talk-v1"];

/// One call's startup, as offsets in milliseconds from the moment the reader pressed Start.
///
/// Every phase is optional because a call can end, fail, or skip one (a typed call never opens a
/// microphone and never plays audio). The field set IS the allowlist: an unknown field is a 400,
/// not a silently dropped key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartupTiming {
    /// Wire protocol of the call.
    pub protocol: String,
    /// Whether the call was typed (no microphone, no playback).
    #[serde(default)]
    pub chat: bool,
    /// The session descriptor came back from `GET /api/v1/voice-session`.
    pub session_acquired: Option<u32>,
    /// The microphone and audio output were ready.
    pub microphone_ready: Option<u32>,
    /// The provider's WebSocket opened.
    pub socket_open: Option<u32>,
    /// The provider said the session is ready.
    pub provider_ready: Option<u32>,
    /// The first assistant transcript text arrived.
    pub greeting_text: Option<u32>,
    /// The first assistant audio frame arrived.
    pub greeting_audio_received: Option<u32>,
    /// The first assistant audio frame was scheduled to start playing.
    pub greeting_audible: Option<u32>,
}

impl StartupTiming {
    /// The phases in the order a healthy call reaches them.
    #[must_use]
    pub fn phases(&self) -> [(&'static str, Option<u32>); 7] {
        [
            ("session_acquired", self.session_acquired),
            ("microphone_ready", self.microphone_ready),
            ("socket_open", self.socket_open),
            ("provider_ready", self.provider_ready),
            ("greeting_text", self.greeting_text),
            ("greeting_audio_received", self.greeting_audio_received),
            ("greeting_audible", self.greeting_audible),
        ]
    }

    /// Refuse anything that is not a plausible measurement.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the offending field.
    pub fn validate(&self) -> Result<(), String> {
        if !PROTOCOLS.contains(&self.protocol.as_str()) {
            return Err("protocol must name a supported voice protocol".to_owned());
        }
        for (name, value) in self.phases() {
            if value.is_some_and(|ms| ms > MAX_PHASE_MS) {
                return Err(format!("{name} is longer than {MAX_PHASE_MS} ms"));
            }
        }
        Ok(())
    }

    /// The single log line: phase names and integers, and nothing anybody said.
    #[must_use]
    pub fn log_fields(&self) -> String {
        let mut fields = format!("protocol={} chat={}", self.protocol, self.chat);
        for (name, value) in self.phases() {
            match value {
                Some(ms) => fields.push_str(&format!(" {name}={ms}")),
                None => fields.push_str(&format!(" {name}=-")),
            }
        }
        // The phase that dominated, so a log reader does not have to subtract.
        let mut previous = 0;
        let mut slowest: Option<(&str, u32)> = None;
        for (name, value) in self.phases() {
            if let Some(ms) = value {
                let step = ms.saturating_sub(previous);
                if slowest.is_none_or(|(_, worst)| step > worst) {
                    slowest = Some((name, step));
                }
                previous = previous.max(ms);
            }
        }
        if let Some((name, step)) = slowest {
            fields.push_str(&format!(" slowest={name}:{step}"));
        }
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChannelId, ChannelInfo};

    #[test]
    fn prompt_fingerprint_is_pinned_to_its_version() {
        for (write, pinned) in [
            (false, PINNED_READ_FINGERPRINT),
            (true, PINNED_WRITE_FINGERPRINT),
        ] {
            assert_eq!(
                prompt(write).fingerprint,
                pinned,
                "a file under prompts/ changed: bump PROMPT_VERSION and update the pinned \
                 {} fingerprint to the value on the left",
                prompt(write).variant
            );
        }
    }

    /// The load-bearing phrases of the send rules. None of them may reach a read-only caller.
    const SEND_RULES: [&str; 5] = [
        "only when the user explicitly asks you to",
        "do not ask for it again",
        "only when the human explicitly requests it",
        "exact provider mention",
        "You can also send",
    ];

    #[test]
    fn prompt_is_present_and_carries_its_load_bearing_rules() {
        for write in [false, true] {
            let text = prompt(write).text;
            assert!(
                text.len() > 1_000,
                "the prompt must not be silently emptied"
            );
            for rule in [
                "summary of the",
                "refute it in a later",
                "Do not read long hashes",
                "Eastern Time",
                "search semantically",
                "say so briefly and wait",
                "untrusted data",
                "Do not repeatedly ask whether the user is there",
            ] {
                assert!(
                    text.contains(rule),
                    "prompt lost the rule containing {rule:?}"
                );
            }
        }
        let write = prompt(true).text;
        for rule in SEND_RULES {
            assert!(
                write.contains(rule),
                "send prompt lost the rule containing {rule:?}"
            );
        }
    }

    #[test]
    fn the_base_prompt_says_nothing_about_sending() {
        // Anything the base says about sending reaches a read-only caller too.
        let lower = SYSTEM_PROMPT.to_lowercase();
        for word in ["send", "post", "reply", "route", "mention"] {
            assert!(!lower.contains(word), "the base prompt mentions {word:?}");
        }
    }

    #[test]
    fn the_read_prompt_carries_no_send_instruction_and_says_it_cannot_send() {
        let read = prompt(false);
        assert_eq!(read.variant, "read");
        for rule in SEND_RULES {
            assert!(
                !read.text.contains(rule),
                "a read-only caller was told {rule:?}"
            );
        }
        assert!(read.text.contains("cannot post or send"), "{}", read.text);
        assert!(
            read.text.contains("cannot send from this connection"),
            "{}",
            read.text
        );
        assert_ne!(read.fingerprint, prompt(true).fingerprint);
    }

    #[test]
    fn prompt_names_no_particular_provider() {
        for write in [false, true] {
            let lower = prompt(write).text.to_lowercase();
            for name in [
                "discord",
                "google",
                "slack",
                "elevenlabs",
                "meta",
                "workplace",
                "<@",
            ] {
                assert!(!lower.contains(name), "prompt names {name:?}");
            }
        }
    }

    fn manifest() -> Vec<ToolDescriptor> {
        crate::mcp::tool_manifest(&[ChannelInfo {
            id: ChannelId("111".to_owned()),
            label: "team".to_owned(),
            writable: true,
            alias: None,
            added: false,
        }])
    }

    #[test]
    fn read_scope_sees_write_tools_as_unavailable() {
        let profile = profile(&manifest(), Scope::Read, "Chat", "America/New_York");
        assert_eq!(profile.scope, "read");
        assert!(!profile.write_available);
        assert_eq!(
            profile.prompt,
            prompt(false),
            "a read caller got the send rules"
        );
        let post = profile
            .tools
            .iter()
            .find(|t| t.name == "post_reply")
            .expect("post_reply listed");
        assert_eq!(post.access, ToolAccess::Write);
        assert!(post.requires_confirmation);
        assert!(!post.available);
        assert!(profile
            .tools
            .iter()
            .filter(|t| t.access == ToolAccess::Read)
            .all(|t| t.available && !t.requires_confirmation));
        assert!(
            profile.tools.iter().all(|t| t.name != "ask_agent"),
            "a tool the MCP endpoint never lists must not be advertised"
        );
    }

    #[test]
    fn write_scope_makes_the_send_tool_available() {
        let profile = profile(&manifest(), Scope::Write, "Chat", "America/New_York");
        assert!(profile.write_available);
        assert_eq!(profile.prompt, prompt(true));
        assert!(profile.tools.iter().all(|t| t.available));
    }

    #[test]
    fn timing_log_line_is_names_and_numbers_only() {
        let timing = StartupTiming {
            protocol: "vibe-talk-v1".to_owned(),
            chat: false,
            session_acquired: Some(40),
            microphone_ready: Some(300),
            socket_open: Some(420),
            provider_ready: Some(2_900),
            greeting_text: Some(4_100),
            greeting_audio_received: Some(4_000),
            greeting_audible: Some(4_050),
        };
        timing.validate().expect("valid");
        let line = timing.log_fields();
        assert_eq!(
            line,
            "protocol=vibe-talk-v1 chat=false session_acquired=40 microphone_ready=300 \
             socket_open=420 provider_ready=2900 greeting_text=4100 \
             greeting_audio_received=4000 greeting_audible=4050 slowest=provider_ready:2480"
        );
    }

    #[test]
    fn timing_refuses_unknown_fields_protocols_and_absurd_values() {
        let smuggled = serde_json::json!({ "protocol": "vibe-talk-v1", "text": "hello" });
        assert!(serde_json::from_value::<StartupTiming>(smuggled).is_err());
        let unknown = StartupTiming {
            protocol: "hello world".to_owned(),
            ..StartupTiming::default()
        };
        assert!(unknown.validate().is_err());
        let absurd = StartupTiming {
            protocol: "elevenlabs".to_owned(),
            socket_open: Some(MAX_PHASE_MS + 1),
            ..StartupTiming::default()
        };
        assert!(absurd.validate().is_err());
    }
}
