//! The owner's own words for the voice agent: where they are kept, what Settings shows of them,
//! and how they reach a call.
//!
//! `#272 voice-agent-instructions`. The owner asked for "a section for additional instructions that
//! the user can fill in" and "a collapse section to actually edit the system prompt", and guessed
//! that a hosted agent would refuse the second while still taking the first. So there are two
//! texts ([`OwnerPrompt`]):
//!
//! * **Additional instructions**, added to every call: "call me Sam", "keep it under three
//!   sentences".
//! * **The base prompt itself**, his own edited copy of [`voice_agent::SYSTEM_PROMPT`].
//!
//! # What neither can change
//!
//! The section that says what the agent can do — read only, send after asking, or send at once —
//! is not his to edit. [`voice_agent::prompt_with`] still chooses it from the credential and the
//! send setting and puts it after his words, and the server enforces both whatever the prompt says:
//! a read token cannot post, and with "Ask me before the voice agent sends" on, `post_reply` only
//! proposes. Editing the prompt cannot grant a power the token lacks.
//!
//! # Where they live
//!
//! One row in the store, written only by a write-scope `PUT /api/v1/voice-prompt` that Settings
//! makes. No MCP tool reaches it — a model that could rewrite its own instructions could undo the
//! rules about untrusted chat text — and [`crate::mcp::protocol::dispatch`] refuses any tool name
//! the manifest does not list. They are readable at read scope, because the assembled prompt is:
//! a bridge holding the read token fetches it.
//!
//! # It fails toward the shipped prompt
//!
//! [`current`] answers the shipped prompt whenever the store cannot say what the owner saved: a
//! broken store must never be what changes what the agent is told.
//!
//! # How they reach a call
//!
//! * **A deployment-managed bridge** fetches `GET /api/v1/voice-agent` as each call opens; the
//!   prompt there is assembled with both texts, so his next call has them.
//! * **A hosted agent** is configured in its vendor's console, and the page opens its socket. So
//!   the page hands his words over as the socket opens, in the way [`for_session`] chooses: the
//!   whole assembled prompt as an override when the agent's security settings allow one, and
//!   otherwise his instructions alone, as a note right after the conversation starts. An edited
//!   base cannot reach such an agent at all, and the session says so rather than pretending.
//!
//! The override is sent only when the agent's own configuration says it is allowed. One it does
//! not allow is believed — from the vendor's documentation, not from a call observed here — to end
//! the conversation rather than be ignored, so "could not tell" is treated as "not allowed", and
//! the note is the fallback, never the override.

use std::time::Duration;

use crate::config::ConversationBackend;
use crate::contract::{PromptDelivery, PromptTransport, SessionPrompt, VoicePromptSettings};
use crate::ops::OpError;
use crate::send_approval;
use crate::state::AppState;
use crate::store::StoreError;
use crate::voice_agent::{self, EditedPrompt, OwnerPrompt, Variant};

/// The owner's words in force, as of this call. Read on every profile and every voice session, so
/// a save in Settings reaches the next call.
///
/// What he saved, or the shipped prompt when he saved nothing or no store is configured. Any other
/// store failure is the shipped prompt too, with a warning.
pub async fn current(state: &AppState) -> OwnerPrompt {
    match state.store.owner_prompt().await {
        Ok(Some(prompt)) => prompt,
        Ok(None) | Err(StoreError::Unavailable(_)) => OwnerPrompt::default(),
        Err(error) => {
            tracing::warn!(
                %error,
                "could not read the owner's instructions for the voice agent; using the shipped prompt"
            );
            OwnerPrompt::default()
        }
    }
}

/// How the owner's words reach this deployment's voice agent: through the profile a bridge fetches,
/// or through the page, for a hosted agent.
#[must_use]
pub fn delivery(state: &AppState) -> PromptDelivery {
    match state.config.conversation.backend {
        ConversationBackend::ElevenLabs => PromptDelivery::Page,
        ConversationBackend::WebSocket => PromptDelivery::Profile,
    }
}

fn settings(state: &AppState, owner: &OwnerPrompt, changeable: bool) -> VoicePromptSettings {
    VoicePromptSettings {
        instructions: owner.instructions.clone(),
        system_prompt: owner.base().to_owned(),
        system_prompt_edited: owner.system_prompt.is_some(),
        edited_from_version: owner
            .system_prompt
            .as_ref()
            .map(|edited| edited.from_version),
        prompt_version: voice_agent::PROMPT_VERSION,
        max_instructions_chars: u32::try_from(voice_agent::MAX_INSTRUCTIONS_CHARS)
            .unwrap_or(u32::MAX),
        max_system_prompt_chars: u32::try_from(voice_agent::MAX_SYSTEM_PROMPT_CHARS)
            .unwrap_or(u32::MAX),
        changeable,
        delivery: delivery(state),
    }
}

/// The owner's words as Settings shows them, and whether this server can keep a change.
///
/// `changeable` is false when no store is configured, and when the configured one cannot be read —
/// a page must not offer an editor whose saved text it cannot know.
pub async fn view(state: &AppState) -> VoicePromptSettings {
    let (owner, changeable) = match state.store.owner_prompt().await {
        Ok(Some(prompt)) => (prompt, true),
        Ok(None) => (OwnerPrompt::default(), true),
        Err(StoreError::Unavailable(_)) => (OwnerPrompt::default(), false),
        Err(error) => {
            tracing::warn!(%error, "could not read the voice agent's instructions for Settings");
            (OwnerPrompt::default(), false)
        }
    };
    settings(state, &owner, changeable)
}

/// One save from Settings. A field left `None` keeps that text as it is, so the two editors save
/// independently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// The new additional instructions; empty removes them.
    pub instructions: Option<String>,
    /// The new base prompt; empty, or the shipped prompt itself, restores the shipped one.
    pub system_prompt: Option<String>,
}

/// Record a change and return the owner's words as they now stand. **Write scope**, and the
/// owner's act alone: there is no tool for it.
///
/// # Errors
///
/// [`OpError::Store`] with [`StoreError::TooLarge`] for a text over its limit, and when no store is
/// configured, the store cannot be read (a change is applied to what is saved, and a save over an
/// unknown row could drop the other text), or the write fails.
pub async fn set(state: &AppState, change: Change) -> Result<VoicePromptSettings, OpError> {
    let instructions = change
        .instructions
        .as_deref()
        .map(voice_agent::clean_instructions)
        .transpose()
        .map_err(StoreError::TooLarge)?;
    let system_prompt = change
        .system_prompt
        .as_deref()
        .map(voice_agent::clean_system_prompt)
        .transpose()
        .map_err(StoreError::TooLarge)?;
    let mut owner = state.store.owner_prompt().await?.unwrap_or_default();
    if let Some(instructions) = instructions {
        owner.instructions = instructions;
    }
    if let Some(text) = system_prompt {
        owner.system_prompt = text.map(|text| {
            // The version shipped when he made THIS text: saving the same words again keeps it,
            // so a re-save does not hide that the shipped prompt has moved on.
            let from_version = match &owner.system_prompt {
                Some(kept) if kept.text == text => kept.from_version,
                _ => voice_agent::PROMPT_VERSION,
            };
            EditedPrompt { text, from_version }
        });
    }
    let stored = state.store.set_owner_prompt(&owner).await?;
    // Sizes and a flag, and none of his words: the access log is content-free.
    tracing::info!(
        target: "vibe_talk::access",
        event = "changed",
        instructions_chars = stored.instructions.chars().count(),
        system_prompt_edited = stored.system_prompt.is_some(),
        "voice_prompt"
    );
    Ok(settings(state, &stored, true))
}

/// The longest a call's start waits to learn whether the hosted agent allows its prompt to be
/// replaced. Asked beside the session mint rather than after it, so this is the most it can add;
/// past it the answer counts as "not allowed", which sends the instructions as a note instead.
pub const AGENT_CHECK_DEADLINE: Duration = Duration::from_millis(2_500);

/// What the page should hand a hosted agent as this call opens, or `None` when there is nothing:
/// a provider that reads the profile instead, or an owner who has written nothing.
///
/// The hosted agent is given the prompt for a credential that may post, under the send setting —
/// the voice session that carries this needs the write token, and an agent that can propose is the
/// one the voice page exists to confirm for. An operator who gives a hosted agent only the read
/// token should not allow it a prompt override; the QUICKSTART says so.
pub async fn for_session(state: &AppState) -> Option<SessionPrompt> {
    if delivery(state) != PromptDelivery::Page {
        return None;
    }
    let owner = current(state).await;
    if owner.is_shipped() {
        return None;
    }
    let allowed = match tokio::time::timeout(
        AGENT_CHECK_DEADLINE,
        state.elevenlabs.agent_profile(&state.config.elevenlabs),
    )
    .await
    {
        Ok(Ok(profile)) => profile.prompt_override,
        Ok(Err(error)) => {
            tracing::warn!(
                %error,
                "could not read whether the voice agent allows its prompt to be replaced; \
                 treating it as not allowed"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                "the voice agent's configuration did not answer in time; treating a prompt \
                 override as not allowed"
            );
            None
        }
    };
    let variant = Variant::for_caller(true, send_approval::current(state).await);
    Some(session_prompt(variant, &owner, allowed))
}

/// The choice [`for_session`] makes, once it knows what the agent allows.
#[must_use]
pub fn session_prompt(
    variant: Variant,
    owner: &OwnerPrompt,
    allowed: Option<bool>,
) -> SessionPrompt {
    if allowed == Some(true) {
        let assembled = voice_agent::prompt_with(variant, owner);
        return SessionPrompt {
            transport: PromptTransport::PromptOverride,
            text: assembled.text,
            system_prompt_applied: assembled.system_prompt_edited,
            instructions_applied: assembled.additional_instructions,
            override_allowed: allowed,
        };
    }
    match voice_agent::owner_note(owner) {
        Some(note) => SessionPrompt {
            transport: PromptTransport::ContextualUpdate,
            text: note,
            system_prompt_applied: false,
            instructions_applied: true,
            override_allowed: allowed,
        },
        None => SessionPrompt {
            transport: PromptTransport::NotSent,
            text: String::new(),
            system_prompt_applied: false,
            instructions_applied: false,
            override_allowed: allowed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StateStore as _;
    use crate::testing;
    use std::sync::Arc;

    fn owner(instructions: &str, base: Option<&str>) -> OwnerPrompt {
        OwnerPrompt {
            instructions: instructions.to_owned(),
            system_prompt: base.map(|text| EditedPrompt {
                text: text.to_owned(),
                from_version: voice_agent::PROMPT_VERSION,
            }),
        }
    }

    fn change(instructions: Option<&str>, system_prompt: Option<&str>) -> Change {
        Change {
            instructions: instructions.map(str::to_owned),
            system_prompt: system_prompt.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn with_nothing_saved_the_agent_gets_the_shipped_prompt_and_settings_can_save() {
        let (state, _discord, _store) = testing::state_with_store();
        assert!(current(&state).await.is_shipped());
        let shown = view(&state).await;
        assert!(shown.changeable);
        assert_eq!(shown.instructions, "");
        assert_eq!(shown.system_prompt, voice_agent::SYSTEM_PROMPT);
        assert!(!shown.system_prompt_edited);
        assert_eq!(shown.edited_from_version, None);
        assert_eq!(shown.prompt_version, voice_agent::PROMPT_VERSION);
        assert_eq!(shown.max_instructions_chars, 4_000);
    }

    #[tokio::test]
    async fn the_two_texts_save_independently_and_an_empty_box_restores() {
        let (state, _discord, _store) = testing::state_with_store();
        let saved = set(&state, change(Some("Call me Sam.\r\n"), None))
            .await
            .expect("save");
        assert_eq!(saved.instructions, "Call me Sam.");
        assert!(!saved.system_prompt_edited);

        let saved = set(&state, change(None, Some("You read me the chat.")))
            .await
            .expect("save");
        assert_eq!(
            saved.instructions, "Call me Sam.",
            "the other text was lost"
        );
        assert!(saved.system_prompt_edited);
        assert_eq!(saved.system_prompt, "You read me the chat.");
        assert_eq!(saved.edited_from_version, Some(voice_agent::PROMPT_VERSION));
        assert_eq!(
            current(&state).await,
            owner("Call me Sam.", Some("You read me the chat."))
        );

        // The shipped prompt pasted back is not an edit, and neither is an empty box.
        for restore in [String::new(), voice_agent::SYSTEM_PROMPT.to_owned()] {
            set(&state, change(None, Some("mine"))).await.expect("save");
            let restored = set(&state, change(None, Some(&restore)))
                .await
                .expect("save");
            assert!(!restored.system_prompt_edited);
            assert_eq!(restored.system_prompt, voice_agent::SYSTEM_PROMPT);
            assert_eq!(restored.instructions, "Call me Sam.");
        }
        let cleared = set(&state, change(Some(""), None)).await.expect("save");
        assert_eq!(cleared.instructions, "");
        assert!(current(&state).await.is_shipped());
    }

    #[tokio::test]
    async fn an_edit_keeps_the_version_it_was_made_from_until_its_words_change() {
        let (state, _discord, store) = testing::state_with_store();
        store
            .set_owner_prompt(&OwnerPrompt {
                instructions: String::new(),
                system_prompt: Some(EditedPrompt {
                    text: "mine".to_owned(),
                    from_version: 3,
                }),
            })
            .await
            .expect("seed");
        let same = set(&state, change(Some("Be brief."), Some("mine")))
            .await
            .expect("save");
        assert_eq!(
            same.edited_from_version,
            Some(3),
            "a re-save hid an older edit"
        );
        let new = set(&state, change(None, Some("mine, changed")))
            .await
            .expect("save");
        assert_eq!(new.edited_from_version, Some(voice_agent::PROMPT_VERSION));
    }

    #[tokio::test]
    async fn a_text_over_its_limit_is_refused_and_nothing_is_saved() {
        let (state, _discord, _store) = testing::state_with_store();
        let long = "x".repeat(voice_agent::MAX_INSTRUCTIONS_CHARS + 1);
        let refused = set(&state, change(Some(&long), Some("mine")))
            .await
            .expect_err("too long");
        assert_eq!(refused.code(), "too_large");
        assert!(
            current(&state).await.is_shipped(),
            "half a refused save was kept"
        );
    }

    #[tokio::test]
    async fn no_store_means_the_shipped_prompt_and_an_editor_that_cannot_save() {
        let (state, _discord) =
            testing::state_with(Arc::new(crate::store::disabled::DisabledStore));
        assert!(current(&state).await.is_shipped());
        assert!(!view(&state).await.changeable);
        assert_eq!(
            set(&state, change(Some("Be brief."), None))
                .await
                .expect_err("nowhere to keep it")
                .code(),
            "storage_not_configured"
        );
    }

    #[tokio::test]
    async fn a_store_that_fails_to_answer_gives_the_shipped_prompt_and_refuses_a_save() {
        let (state, _discord, store) = testing::state_with_store();
        store
            .set_owner_prompt(&owner("Be brief.", Some("mine")))
            .await
            .expect("seed");
        store.fail_next("the disk is gone");
        assert!(
            current(&state).await.is_shipped(),
            "a broken store changed the prompt"
        );
        store.fail_next("the disk is gone");
        assert!(!view(&state).await.changeable);
        store.fail_next("the disk is gone");
        assert!(set(&state, change(None, Some("other"))).await.is_err());
        assert_eq!(current(&state).await, owner("Be brief.", Some("mine")));
    }

    #[test]
    fn an_agent_that_allows_it_gets_the_whole_prompt_with_the_section_last() {
        let mine = owner("Call me Sam.", Some("You read me the chat."));
        for variant in [Variant::WriteConfirm, Variant::WriteDirect] {
            let sent = session_prompt(variant, &mine, Some(true));
            assert_eq!(sent.transport, PromptTransport::PromptOverride);
            assert_eq!(sent.text, voice_agent::prompt_with(variant, &mine).text);
            assert!(sent.system_prompt_applied && sent.instructions_applied);
            assert_eq!(sent.override_allowed, Some(true));
        }
    }

    #[test]
    fn an_agent_that_refuses_or_cannot_say_gets_the_instructions_as_a_note_and_never_an_override() {
        let mine = owner("Call me Sam.", Some("You read me the chat."));
        for allowed in [Some(false), None] {
            let sent = session_prompt(Variant::WriteConfirm, &mine, allowed);
            assert_eq!(
                sent.transport,
                PromptTransport::ContextualUpdate,
                "{allowed:?}"
            );
            assert_eq!(
                sent.text,
                format!("{}\nCall me Sam.", voice_agent::OWNER_INSTRUCTIONS_HEADING)
            );
            assert!(sent.instructions_applied);
            assert!(
                !sent.system_prompt_applied,
                "an edit it cannot take was reported applied"
            );
            assert!(
                !sent.text.contains("You read me the chat."),
                "the edited base went out as a note"
            );
            assert_eq!(sent.override_allowed, allowed);

            // Only an edited base, and nowhere to put it: nothing goes, and it says so.
            let none = session_prompt(Variant::WriteConfirm, &owner("", Some("mine")), allowed);
            assert_eq!(none.transport, PromptTransport::NotSent);
            assert!(none.text.is_empty());
            assert!(!none.system_prompt_applied && !none.instructions_applied);
        }
    }

    #[tokio::test]
    async fn a_call_carries_nothing_when_nothing_was_written_or_a_bridge_reads_the_profile() {
        let (state, _discord, _store) = testing::state_with_store();
        assert_eq!(delivery(&state), PromptDelivery::Page);
        assert_eq!(
            for_session(&state).await,
            None,
            "the shipped prompt needs no handing over"
        );

        let toml = format!(
            "{}\n[conversation]\nbackend = \"websocket\"\nwebsocket_url = \"/ws\"\n",
            testing::config_toml()
        );
        let (bridge, _discord, store) = testing::state_with_store_from_toml(&toml);
        store
            .set_owner_prompt(&owner("Call me Sam.", None))
            .await
            .expect("seed");
        assert_eq!(delivery(&bridge), PromptDelivery::Profile);
        assert_eq!(
            for_session(&bridge).await,
            None,
            "a bridge reads the profile; the page must not send the words twice"
        );
    }
}
