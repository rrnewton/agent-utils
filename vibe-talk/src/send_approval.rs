//! "Ask me before the voice agent sends": the owner's one switch over what `post_reply` does.
//!
//! `#264 voice-send-approval`. Since `#34 voice-chat-write-confirm` the voice agent's `post_reply`
//! has never posted anything: it leaves one pending proposal ([`crate::post_gate`]) that the owner
//! confirms outside the model's tools. That is the right default, and it stays the default. But the
//! owner drives with this, and for him "tell the coding agent to rebase" followed by a read-back
//! and a question is friction he does not want. So the confirmation is a setting he owns:
//!
//! * **On ([`Approval::Ask`], the shipped default).** Exactly the `#34` flow. The agent reads the
//!   draft back and asks "Send it?"; the voice page sends it when he taps Send, or when the page
//!   itself — never the model — hears a plain yes right after an uninterrupted read-back.
//! * **Off ([`Approval::Direct`]).** `post_reply` posts at once, through [`crate::ops::reply`] —
//!   the same function a confirmed proposal is posted with, so the allowlist, the write policy,
//!   the emptiness check and long-message splitting are the ones every other send gets — and the
//!   model is told `SENT.` with what went where, or why it did not.
//!
//! # Where the choice lives
//!
//! The configuration file's `[conversation] ask_before_send` is the SHIPPED default; the owner's
//! own choice is one row in the store, written only by a write-scope `PUT /api/v1/voice-settings`
//! that Settings makes. No MCP tool can reach it — a model that could turn confirmation off could
//! then post anything a message it read asked for — and [`crate::mcp::protocol::dispatch`] refuses
//! any tool name the manifest does not list.
//!
//! # It fails toward asking
//!
//! [`current`] answers [`Approval::Ask`] whenever the store cannot say what the owner chose. A
//! store that is not configured at all is different: there is no choice to lose, so the configured
//! default applies, exactly as it does for the noise rules.
//!
//! # Why this is its own module
//!
//! The direct send calls [`crate::ops::reply`] and nothing below it. Splitting a long post is being
//! reworked inside `ops::reply_scoped`, and keeping this out of that function means there is still
//! exactly one place that decides how a long message goes out.

use crate::model::{ChannelInfo, Message};
use crate::ops::{self, OpError};
use crate::state::AppState;
use crate::store::StoreError;

/// What `post_reply` does with a message right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    /// Propose it and wait for the owner's confirmation. The shipped default.
    Ask,
    /// Post it at once. The owner turned confirmation off.
    Direct,
}

impl Approval {
    /// The approval that a stored or configured `ask_before_send` means.
    #[must_use]
    pub fn from_ask(ask: bool) -> Self {
        if ask {
            Self::Ask
        } else {
            Self::Direct
        }
    }

    /// Whether the owner is asked first.
    #[must_use]
    pub fn asks(self) -> bool {
        self == Self::Ask
    }
}

/// The approval in force, as of this call. Read on every `tools/list`, profile and `post_reply`,
/// so a change in Settings reaches the server's behaviour at once.
///
/// A saved choice wins. No choice, or no store configured, is the configuration file's default.
/// Any other store failure is [`Approval::Ask`], with a warning: a broken store must never be what
/// turns confirmation off.
pub async fn current(state: &AppState) -> Approval {
    match state.store.ask_before_send().await {
        Ok(Some(ask)) => Approval::from_ask(ask),
        Ok(None) | Err(StoreError::Unavailable(_)) => {
            Approval::from_ask(state.config.conversation.ask_before_send)
        }
        Err(error) => {
            tracing::warn!(
                %error,
                "could not read whether the voice agent must ask before it sends; asking"
            );
            Approval::Ask
        }
    }
}

/// The setting as Settings shows it: the value in force, and whether this server can change it.
///
/// `changeable` is false when no store is configured, and when the configured one cannot be read —
/// a page must not offer a switch whose saved position it cannot know.
pub async fn view(state: &AppState) -> crate::contract::VoiceSettings {
    let (ask_before_send, changeable) = match state.store.ask_before_send().await {
        Ok(Some(ask)) => (ask, true),
        Ok(None) => (state.config.conversation.ask_before_send, true),
        Err(StoreError::Unavailable(_)) => (state.config.conversation.ask_before_send, false),
        Err(error) => {
            tracing::warn!(%error, "could not read the voice settings for Settings; asking");
            (true, false)
        }
    };
    crate::contract::VoiceSettings {
        ask_before_send,
        changeable,
    }
}

/// Record the owner's choice and return the setting as it now stands. **Write scope**, and the
/// owner's act alone: there is no tool for it.
///
/// # Errors
///
/// [`OpError::Store`] when no store is configured or the write fails.
pub async fn set(state: &AppState, ask: bool) -> Result<crate::contract::VoiceSettings, OpError> {
    let stored = state.store.set_ask_before_send(ask).await?;
    // The boolean and nothing else. Settings is the only writer, so this line is the record of the
    // owner changing his mind, and the time on it is when.
    tracing::info!(
        target: "vibe_talk::access",
        event = "changed",
        ask_before_send = stored,
        "voice_settings"
    );
    Ok(crate::contract::VoiceSettings {
        ask_before_send: stored,
        changeable: true,
    })
}

/// What one `post_reply` did.
#[derive(Debug)]
pub enum VoicePost {
    /// Proposed for the owner's confirmation; nothing was posted.
    Proposed {
        /// The channel it would go to.
        channel: ChannelInfo,
        /// The proposal now pending.
        proposal: crate::post_gate::Proposal,
    },
    /// Posted at once.
    Sent {
        /// The channel it went to.
        channel: ChannelInfo,
        /// A number correlating the log line; drawn from the same counter as proposals.
        serial: u64,
        /// The first message as the chat service accepted it.
        posted: Message,
        /// Every part, in order.
        parts: Vec<Message>,
        /// The exact text posted.
        text: String,
        /// The message it answered, if any.
        reply_to: Option<String>,
    },
}

/// `post_reply`, as the setting says: propose it, or post it now. The one function the MCP tool and
/// `POST /api/v1/post-proposals` both call, so a client that maps the tool onto the REST route gets
/// the same behaviour and hands its model the same words.
///
/// # Errors
///
/// Whatever [`ops::propose_reply`] or [`ops::reply`] refuses with — the allowlist, the write
/// policy, an empty message — and, for a direct send, the chat service's own failure, including
/// [`OpError::PartiallyPosted`] when part of a long message went out.
pub async fn voice_post(
    state: &AppState,
    channel_id: &str,
    text: &str,
    reply_to: Option<&str>,
) -> Result<VoicePost, OpError> {
    match current(state).await {
        Approval::Ask => {
            let (channel, proposal) = ops::propose_reply(state, channel_id, text, reply_to).await?;
            Ok(VoicePost::Proposed { channel, proposal })
        }
        Approval::Direct => {
            let serial = state.post_gate.number();
            match ops::reply(state, channel_id, text, reply_to).await {
                Ok((channel, posted, parts)) => {
                    crate::access::post_gate(
                        "sent_direct",
                        Some(serial),
                        Some(channel.id.as_str()),
                        Some(text.chars().count()),
                        None,
                        None,
                    );
                    Ok(VoicePost::Sent {
                        channel,
                        serial,
                        posted,
                        parts,
                        text: text.to_owned(),
                        reply_to: reply_to.map(str::to_owned),
                    })
                }
                Err(error) => {
                    crate::access::post_gate(
                        "direct_failed",
                        Some(serial),
                        Some(channel_id),
                        Some(text.chars().count()),
                        Some(error.code()),
                        None,
                    );
                    Err(error)
                }
            }
        }
    }
}

/// What a model is told after a direct send: `SENT.` first, then what went where, in words it can
/// repeat, and that calling again would post again.
#[must_use]
pub fn sent_notice(
    channel: &ChannelInfo,
    text: &str,
    reply_to: Option<&str>,
    parts: usize,
) -> String {
    let target = match reply_to {
        Some(message) => format!(" as a reply to message {message}"),
        None => String::new(),
    };
    let split = if parts > 1 {
        format!("\nIt was long, so it went as {parts} messages.")
    } else {
        String::new()
    };
    format!(
        "SENT. Posted to {channel} (id {id}){target} as the owner's bot.\n\
         Exact text posted: {text}{split}\n\
         Tell the speaker it was sent. Do not call post_reply again for this message; that would \
         post it again.",
        channel = channel.display_name(),
        id = channel.id,
    )
}

/// What goes in front of a `post_reply` failure, so the first word the model reads says whether
/// anything reached the channel. True in both modes: a refused proposal sends nothing either.
#[must_use]
pub fn failure_prefix(error: &OpError) -> &'static str {
    match error {
        OpError::PartiallyPosted { posted, .. } if *posted > 0 => {
            "PARTLY SENT. The rest was NOT sent; do not send the whole message again. "
        }
        _ => "NOT SENT. ",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StateStore as _;
    use crate::testing::{self, WRITE_CHANNEL};
    use std::sync::Arc;

    fn direct_toml() -> String {
        format!(
            "{}\n[conversation]\nask_before_send = false\n",
            testing::config_toml()
        )
    }

    #[tokio::test]
    async fn with_nothing_chosen_the_shipped_default_asks() {
        let (state, _discord, _store) = testing::state_with_store();
        assert_eq!(current(&state).await, Approval::Ask);
        let shown = view(&state).await;
        assert!(shown.ask_before_send && shown.changeable);
    }

    #[tokio::test]
    async fn the_configured_default_applies_until_the_owner_chooses_and_his_choice_wins() {
        let (state, _discord, _store) = testing::state_with_store_from_toml(&direct_toml());
        assert_eq!(current(&state).await, Approval::Direct);
        set(&state, true).await.expect("save");
        assert_eq!(current(&state).await, Approval::Ask);
        set(&state, false).await.expect("save");
        assert_eq!(current(&state).await, Approval::Direct);
    }

    #[tokio::test]
    async fn no_store_means_the_configured_default_and_a_switch_that_cannot_be_moved() {
        let (state, _discord) =
            testing::state_with(Arc::new(crate::store::disabled::DisabledStore));
        assert_eq!(current(&state).await, Approval::Ask);
        let shown = view(&state).await;
        assert!(shown.ask_before_send);
        assert!(!shown.changeable);
        assert_eq!(
            set(&state, false)
                .await
                .expect_err("nowhere to keep it")
                .code(),
            "storage_not_configured"
        );
        assert_eq!(current(&state).await, Approval::Ask);
    }

    #[tokio::test]
    async fn a_store_that_fails_to_answer_asks_even_when_the_default_does_not() {
        let (state, _discord, store) = testing::state_with_store_from_toml(&direct_toml());
        store.set_ask_before_send(false).await.expect("save");
        store.fail_next("the disk is gone");
        assert_eq!(
            current(&state).await,
            Approval::Ask,
            "a broken store turned confirmation off"
        );
        store.fail_next("the disk is gone");
        let shown = view(&state).await;
        assert!(shown.ask_before_send && !shown.changeable);
    }

    #[tokio::test]
    async fn asking_proposes_and_posts_nothing() {
        let (state, discord, _store) = testing::state_with_store();
        let outcome = voice_post(&state, WRITE_CHANNEL, "on it", None)
            .await
            .expect("proposed");
        assert!(matches!(outcome, VoicePost::Proposed { .. }), "{outcome:?}");
        assert!(discord.posted().is_empty(), "asking posted something");
        assert!(state.post_gate.pending().is_some());
    }

    #[tokio::test]
    async fn direct_posts_once_through_the_shared_send_and_leaves_nothing_pending() {
        let (state, discord, _store) = testing::state_with_store_from_toml(&direct_toml());
        let outcome = voice_post(&state, WRITE_CHANNEL, "rebase onto main", Some("42"))
            .await
            .expect("sent");
        let VoicePost::Sent {
            channel,
            parts,
            text,
            reply_to,
            ..
        } = outcome
        else {
            panic!("a direct send proposed instead: {outcome:?}");
        };
        assert_eq!(discord.posted().len(), 1);
        assert!(state.post_gate.pending().is_none());
        let said = sent_notice(&channel, &text, reply_to.as_deref(), parts.len());
        assert!(
            said.starts_with("SENT. Posted to lead team (id 2222222222) as a reply to message 42"),
            "{said}"
        );
        assert!(
            said.contains("Exact text posted: rebase onto main"),
            "{said}"
        );
        assert!(!said.contains("went as"), "{said}");
    }

    #[test]
    fn a_failure_says_first_whether_anything_reached_the_channel() {
        assert_eq!(failure_prefix(&OpError::UnknownChannel), "NOT SENT. ");
        let none = OpError::PartiallyPosted {
            posted: 0,
            unsent: "all of it".to_owned(),
            cause: "down".to_owned(),
            retryable: true,
            resumable: true,
            thread_id: None,
        };
        assert_eq!(failure_prefix(&none), "NOT SENT. ");
        let some = OpError::PartiallyPosted {
            posted: 1,
            unsent: "the rest".to_owned(),
            cause: "down".to_owned(),
            retryable: true,
            resumable: true,
            thread_id: None,
        };
        assert!(failure_prefix(&some).starts_with("PARTLY SENT."));
    }
}
