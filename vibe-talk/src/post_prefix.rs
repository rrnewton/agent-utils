//! The tag this server puts before every message the voice agent posts.
//!
//! `#303 voice-post-prefix`. A message the voice agent posts goes out as the owner's bot, and
//! nothing in it said that a voice agent had relayed it: a reader could not tell the owner's own
//! typed words from words a model heard, drafted and posted for him. So every post the voice agent
//! makes starts with a tag the owner can change in Settings — by default [`DEFAULT_PREFIX`], with
//! its trailing space — and posts he types on the page never carry it.
//!
//! # One place, so no path is missed
//!
//! [`crate::send_approval::voice_post`] is the one function MCP's `post_reply` and
//! `POST /api/v1/post-proposals` both call, whether the owner has the agent ask first or not, and it
//! calls [`apply`] before anything else. Everything after it sees the tagged text:
//!
//! * **A proposal** holds it, so the send card shows it, the read-back the agent is handed says it,
//!   and the commit — a tap on Send or a spoken yes — restates it and posts exactly it. A commit
//!   never passes through `voice_post`, so it is never tagged a second time.
//! * **A direct send** (confirmation off) posts it, and `SENT.` quotes it.
//! * **A reply** carries it whichever way the provider threads replies: the target rides beside the
//!   text, untouched.
//! * **A long post** is cut after tagging (`#263 post-split-per-provider`), so the tag counts toward
//!   the provider's limit — `max_post_bytes`, `max_post_chars` — and lands in the first part only.
//!
//! The page's own posts go to `POST /api/v1/channels/{id}/reply`, which never reaches
//! `voice_post`, so nothing the owner types is tagged.
//!
//! # Once, never twice
//!
//! The model is shown the tagged text — "Exact text:", the read-back, "Exact text posted:" — and
//! may hand it straight back: a correction re-proposed from what it was shown, a retry of a direct
//! send. [`apply`] keeps one tag on a text that already starts with it, so the post carries it once
//! and a retried direct send is still recognised as the same words. The prompt tells the agent
//! that this server adds the tag, so it does not add one of its own.
//!
//! # Where it lives
//!
//! One row in the store, beside the owner's instructions for the agent (`#272
//! voice-agent-instructions`), written only by a write-scope `PUT` or `DELETE
//! /api/v1/voice-post-prefix` that Settings makes. No row is the shipped default; a saved empty
//! tag turns it off, and the two stay apart ([`crate::store::StateStore::post_prefix`]). No MCP
//! tool reaches it: a model that could take its own tag off could pass its words off as the
//! owner's.
//!
//! # It fails toward the tag
//!
//! [`current`] answers the default whenever the store cannot say what the owner saved. Labelling a
//! relayed message is the honest direction to fail in; posting one unlabelled because a disk was
//! briefly unreadable is not.

use crate::contract::VoicePostPrefix;
use crate::ops::OpError;
use crate::state::AppState;
use crate::store::StoreError;

/// The tag on every message the voice agent posts until the owner saves his own: what it is, and
/// who it is from. The trailing space keeps it off the first word.
pub const DEFAULT_PREFIX: &str = "(Voice-agent relayed, from user) ";

/// The longest tag a save may carry, in characters. Short enough that, even at four bytes a
/// character, it fits in the smallest post a provider may be configured to take
/// ([`crate::config::MIN_POST_SIZE`]) with room left for words in a long post's first part.
pub const MAX_PREFIX_CHARS: usize = 100;

// Checked when this compiles, so a limit raised past the smallest post cannot ship.
const _: () = assert!(MAX_PREFIX_CHARS * 4 < crate::config::MIN_POST_SIZE);

/// The tag in force, as of this call: the owner's saved one, which may be empty, or
/// [`DEFAULT_PREFIX`] when he has saved none or no store is configured. Any other store failure is
/// the default too, with a warning — see "It fails toward the tag" above.
pub async fn current(state: &AppState) -> String {
    match state.store.post_prefix().await {
        Ok(Some(prefix)) => prefix,
        Ok(None) | Err(StoreError::Unavailable(_)) => DEFAULT_PREFIX.to_owned(),
        Err(error) => {
            tracing::warn!(
                %error,
                "could not read the tag for the voice agent's posts; using the default"
            );
            DEFAULT_PREFIX.to_owned()
        }
    }
}

/// Check and tidy a tag the owner typed.
///
/// It is one line: a line break or any other control character becomes a space. Leading space goes,
/// because a post does not start with one; trailing space stays, because it is what separates the
/// tag from the first word. A tag of nothing but space is the empty tag, which turns tagging off.
///
/// # Errors
///
/// A sentence naming the limit when the tag is over [`MAX_PREFIX_CHARS`].
pub fn clean(prefix: &str) -> Result<String, String> {
    let one_line: String = prefix
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let tidy = one_line.trim_start();
    let tidy = if tidy.trim_end().is_empty() { "" } else { tidy };
    let length = tidy.chars().count();
    if length > MAX_PREFIX_CHARS {
        return Err(format!(
            "the tag is {length} characters; at most {MAX_PREFIX_CHARS} are kept"
        ));
    }
    Ok(tidy.to_owned())
}

/// A message with the tag on it, and which part of it is the tag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tagged {
    /// What will be posted: the tag, then the words.
    pub text: String,
    /// The start of `text` that this server put there — `text` begins with exactly this — or empty
    /// when it put nothing: tagging is off, or there are no words to tag.
    pub prefix: String,
}

/// Put `prefix` before `text`, once.
///
/// * Nothing is added when the tag is empty or the text holds no words: an empty post is refused
///   downstream as it always was, not posted as a bare tag.
/// * A text that already starts with the tag — ignoring space before it, and the tag's own trailing
///   space — keeps one: the model handed back the text it was shown. A text that is nothing but the
///   tag is no words at all, and comes back empty for the same refusal.
/// * A text whose first line opens a Markdown block — a code fence, a quote, a list item or a
///   heading — gets the tag on a line of its own, so the block still renders as one.
///
/// Leading space before the words is dropped, as the read-back drops it.
#[must_use]
pub fn apply(prefix: &str, text: &str) -> Tagged {
    let tag = prefix.trim_end();
    if tag.is_empty() || text.trim().is_empty() {
        return Tagged {
            text: text.to_owned(),
            prefix: String::new(),
        };
    }
    let words = text.trim_start();
    let words = match words.strip_prefix(tag) {
        Some(rest) if rest.trim().is_empty() => return Tagged::default(),
        Some(rest) => rest.trim_start(),
        None => words,
    };
    let prefix = if opens_a_block(words) {
        format!("{tag}\n")
    } else {
        prefix.to_owned()
    };
    Tagged {
        text: format!("{prefix}{words}"),
        prefix,
    }
}

/// Whether `text`'s first line starts a Markdown block that text in front of it would break: a code
/// fence, a quote, a list item, or a heading.
fn opens_a_block(text: &str) -> bool {
    let first = text.lines().next().unwrap_or_default();
    let heading = first.starts_with('#') && first.trim_start_matches('#').starts_with(' ');
    let numbered = {
        let digits = first.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && (first[digits..].starts_with(". ") || first[digits..].starts_with(") "))
    };
    first.starts_with("```")
        || first.starts_with("~~~")
        || first.starts_with('>')
        || first.starts_with("- ")
        || first.starts_with("* ")
        || first.starts_with("+ ")
        || heading
        || numbered
}

fn view_of(saved: Option<String>, changeable: bool) -> VoicePostPrefix {
    VoicePostPrefix {
        custom: saved.is_some(),
        prefix: saved.unwrap_or_else(|| DEFAULT_PREFIX.to_owned()),
        default_prefix: DEFAULT_PREFIX.to_owned(),
        max_chars: u32::try_from(MAX_PREFIX_CHARS).unwrap_or(u32::MAX),
        changeable,
    }
}

/// The tag as Settings shows it, and whether this server can keep a change.
///
/// `changeable` is false when no store is configured, and when the configured one cannot be read —
/// a page must not offer a field whose saved value it cannot know.
pub async fn view(state: &AppState) -> VoicePostPrefix {
    match state.store.post_prefix().await {
        Ok(saved) => view_of(saved, true),
        Err(StoreError::Unavailable(_)) => view_of(None, false),
        Err(error) => {
            tracing::warn!(%error, "could not read the voice agent's post tag for Settings");
            view_of(None, false)
        }
    }
}

/// Save the owner's tag — `Some`, which may be empty and so turn tagging off — or forget it,
/// `None`, so the default applies again. Returns the tag as it now stands. **Write scope**, and the
/// owner's act alone: there is no tool for it.
///
/// # Errors
///
/// [`OpError::Store`] with [`StoreError::TooLarge`] for a tag over [`MAX_PREFIX_CHARS`], and when
/// no store is configured or the write fails.
pub async fn set(state: &AppState, prefix: Option<&str>) -> Result<VoicePostPrefix, OpError> {
    let cleaned = prefix
        .map(clean)
        .transpose()
        .map_err(StoreError::TooLarge)?;
    let stored = state.store.set_post_prefix(cleaned.as_deref()).await?;
    // Whether he has his own, and how long it is: none of his words, as the access log is
    // content-free.
    tracing::info!(
        target: "vibe_talk::access",
        event = if stored.is_some() { "changed" } else { "reset" },
        prefix_chars = stored.as_deref().map_or(0, |prefix| prefix.chars().count()),
        "voice_post_prefix"
    );
    Ok(view_of(stored, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StateStore as _;
    use crate::testing;
    use std::sync::Arc;

    const TAG: &str = DEFAULT_PREFIX;

    #[test]
    fn the_tag_goes_in_front_once() {
        assert_eq!(
            apply(TAG, "on it"),
            Tagged {
                text: format!("{TAG}on it"),
                prefix: TAG.to_owned(),
            }
        );
        // Handed back as it was shown: still one tag, and still reported as the server's.
        let again = apply(TAG, &format!("{TAG}on it"));
        assert_eq!(again.text, format!("{TAG}on it"));
        assert_eq!(again.prefix, TAG);
        // With space before it, or the tag's own space missing: one tag, as the server writes it.
        assert_eq!(
            apply(TAG, &format!("  {TAG}on it")).text,
            format!("{TAG}on it")
        );
        assert_eq!(
            apply(TAG, "(Voice-agent relayed, from user)on it").text,
            format!("{TAG}on it")
        );
        // Leading space before the words is not kept between the tag and them.
        assert_eq!(apply(TAG, "   on it").text, format!("{TAG}on it"));
    }

    #[test]
    fn no_words_get_no_tag_so_an_empty_post_is_still_refused() {
        for text in ["", "   ", "\n"] {
            let tagged = apply(TAG, text);
            assert_eq!(tagged.text, text);
            assert_eq!(tagged.prefix, "");
        }
        // Nothing but the tag is no words either.
        assert_eq!(apply(TAG, TAG), Tagged::default());
        assert_eq!(apply(TAG, TAG.trim_end()), Tagged::default());
    }

    #[test]
    fn an_empty_tag_adds_nothing_and_changes_nothing() {
        for prefix in ["", "   "] {
            let tagged = apply(prefix, "  on it");
            assert_eq!(tagged.text, "  on it");
            assert_eq!(tagged.prefix, "");
        }
    }

    #[test]
    fn a_tag_with_no_trailing_space_is_used_as_saved() {
        assert_eq!(apply("[relayed]", "on it").text, "[relayed]on it");
        assert_eq!(apply("[relayed]", "[relayed] on it").text, "[relayed]on it");
    }

    #[test]
    fn a_block_keeps_rendering_as_one_with_the_tag_on_its_own_line() {
        for text in [
            "```\ncargo test\n```",
            "> quoted",
            "- one\n- two",
            "* one",
            "1. first",
            "2) second",
            "## Heading",
        ] {
            let tagged = apply(TAG, text);
            assert_eq!(
                tagged.prefix, "(Voice-agent relayed, from user)\n",
                "{text}"
            );
            assert_eq!(tagged.text, format!("{}{text}", tagged.prefix), "{text}");
        }
        // Words that merely start with the same signs are not a block.
        for text in [
            "#303 is fixed",
            "-1 from me",
            "*really* soon",
            "2026 is here",
        ] {
            assert_eq!(apply(TAG, text).prefix, TAG, "{text}");
        }
    }

    #[test]
    fn a_saved_tag_is_one_line_with_its_trailing_space_kept() {
        assert_eq!(clean(TAG), Ok(TAG.to_owned()));
        assert_eq!(clean("  [relayed] "), Ok("[relayed] ".to_owned()));
        assert_eq!(clean("[voice]\nrelay "), Ok("[voice] relay ".to_owned()));
        assert_eq!(clean("   "), Ok(String::new()));
        assert_eq!(clean(""), Ok(String::new()));
        assert_eq!(
            clean(&"é".repeat(MAX_PREFIX_CHARS)).map(|t| t.chars().count()),
            Ok(100)
        );
        let refused = clean(&"x".repeat(MAX_PREFIX_CHARS + 1)).expect_err("too long");
        assert!(refused.contains("at most 100"), "{refused}");
    }

    #[test]
    fn the_default_tag_keeps_off_the_first_word_and_fits_its_own_limit() {
        assert!(DEFAULT_PREFIX.ends_with(' '));
        assert_eq!(clean(DEFAULT_PREFIX).as_deref(), Ok(DEFAULT_PREFIX));
    }

    #[tokio::test]
    async fn unset_is_the_default_saved_empty_is_off_and_reset_brings_the_default_back() {
        let (state, _discord, _store) = testing::state_with_store();
        assert_eq!(current(&state).await, DEFAULT_PREFIX);
        let shown = view(&state).await;
        assert_eq!(shown.prefix, DEFAULT_PREFIX);
        assert!(!shown.custom && shown.changeable);

        let off = set(&state, Some("")).await.expect("save");
        assert_eq!(off.prefix, "");
        assert!(off.custom, "an empty tag read back as never set");
        assert_eq!(current(&state).await, "");

        let own = set(&state, Some(" [voice] ")).await.expect("save");
        assert_eq!(own.prefix, "[voice] ");
        assert_eq!(current(&state).await, "[voice] ");

        let reset = set(&state, None).await.expect("reset");
        assert_eq!(reset.prefix, DEFAULT_PREFIX);
        assert!(!reset.custom);
        assert_eq!(current(&state).await, DEFAULT_PREFIX);
    }

    #[tokio::test]
    async fn a_tag_over_the_limit_is_refused_and_nothing_changes() {
        let (state, _discord, _store) = testing::state_with_store();
        let error = set(&state, Some(&"x".repeat(MAX_PREFIX_CHARS + 1)))
            .await
            .expect_err("too long");
        assert_eq!(error.code(), "too_large");
        assert_eq!(current(&state).await, DEFAULT_PREFIX);
    }

    #[tokio::test]
    async fn no_store_means_the_default_and_a_field_that_cannot_be_changed() {
        let (state, _discord) =
            testing::state_with(Arc::new(crate::store::disabled::DisabledStore));
        assert_eq!(current(&state).await, DEFAULT_PREFIX);
        let shown = view(&state).await;
        assert!(!shown.changeable && !shown.custom);
        assert_eq!(
            set(&state, Some("")).await.expect_err("nowhere").code(),
            "storage_not_configured"
        );
    }

    #[tokio::test]
    async fn a_store_that_fails_to_answer_tags_even_when_the_owner_turned_it_off() {
        let (state, _discord, store) = testing::state_with_store();
        store.set_post_prefix(Some("")).await.expect("save");
        store.fail_next("the disk is gone");
        assert_eq!(current(&state).await, DEFAULT_PREFIX);
        store.fail_next("the disk is gone");
        assert!(!view(&state).await.changeable);
    }
}
