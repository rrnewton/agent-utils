//! The owner's settings that live on the server and are shared by every device.
//! `#457 settings-server-side`.
//!
//! The owner, 2026-10-23: "I THOUGHT all my settings were persisted and server side. …
//! anything with the messaging data model should be server side (like mark read), but
//! something like text size should stay client side. It would be NICE if these corresponded
//! to top-level groups under Settings. Like 'Screen and keys' could say in small/faint font
//! '(client side)' after its title. Everything else server side, and that one section
//! client side."
//!
//! So the division is by Settings section, and this module is the server side of it:
//!
//! * **Screen and keys stays on the device** — text size (`msg-scale`), reading width
//!   (`width`), control-bar placement (`bar-placement`), Enter sends (`enter-sends`),
//!   single keys (`single-keys`), where Slack links open (`slack-open`: whether that app
//!   is installed is this device's own business, `#481 slack-app-deep-link`), and the
//!   composer-grow box beside the composer (`composer-grow`, a screen-layout choice with
//!   no Settings row of its own). None of those keys appears below, and the section's
//!   heading says "(client side)".
//! * **Everything else is here** — one document for the owner (this server has one), a row
//!   per key, **per-key last-writer-wins on the server's clock**. That is the two-party
//!   shape `ai_docs/transient/2026-10-23-replication-model.md` §4 gives small owner
//!   choices: the server serializes writes, `updated_ms` is its one clock, and every page
//!   converges by taking the entry with the newer stamp.
//!
//! Two settings moved section to make the rule true, and the page says so where they sit:
//! **Collapsed message lines** left Screen and keys for Messages (it decides how much of
//! a message is shown, like Combining and Rich headers beside it), so it syncs. The
//! read-aloud pace and voice, the thread pack/unpack choices and sort order, and the
//! hidden link kinds have their controls on the main screen rather than in a Settings
//! section; they are messaging choices, so they sync too. The microphone knobs sit in
//! Voice agent, so by the owner's one-section rule they sync as well.
//!
//! Not settings, and deliberately not here: drafts and the outbox (the owner's unsent
//! words, which the replication model keeps on the device until sent), dismissed sends
//! kept for Undo, the saved message cache (a replica of the server, not a choice), the
//! per-window column layout (`columns`, one window's own by `#281 view-keys`), the
//! read strip's dragged position (`read-steer-offset`, a place on one desk's screen),
//! the active channel and other UI state, the census of who has spoken where (a cache of
//! what was read; the owner's choices about those accounts sync, `#488`), and the token. Settings that
//! were already server-side keep their own homes — voice send approval, the voice
//! prompt and post prefix, noise rules, channel aliases and upstream-clear — and are not
//! duplicated here.
//!
//! A key's value is the exact string the page keeps in `localStorage` under that key.
//! The server stores it unparsed: the page's own `stored…` readers clamp and default
//! whatever they find, on every device, so a value written by a newer page degrades on
//! an older one the way a corrupt local entry always has. One value is also READ here,
//! by the same rule: the thread card lines (`#516 thread-card-max-lines`,
//! [`crate::threads::card_max_lines`]), which decide when a thread is named, and are
//! never refused for their contents. What the server does decide
//! is WHICH keys exist ([`SYNCED_KEYS`]) and how large a value may be
//! ([`MAX_VALUE_BYTES`]): an unknown key is refused, so this cannot become a general
//! key-value store a caller stuffs channel text into. No MCP tool reaches it, for the
//! `#39` reason: a model that could change how its owner's messages are marked and
//! shown could hide them from him.

use serde::{Deserialize, Serialize};

/// Every `localStorage` key whose setting lives on the server, in the order the page
/// declares them. The page's `SYNCED_SETTING_KEYS` in `web/voice.js` must name the same
/// set; a test below reads that file and compares, so the two cannot drift apart
/// silently — a key only the page syncs would be refused here and spin its retry
/// forever, and a key only here would sit unsynced while its section implied otherwise.
pub const SYNCED_KEYS: &[&str] = &[
    // Voice agent: resuming, arriving-message relay, reading aloud, canned prompts, mic.
    "vibe-talk.voice.resume",
    "vibe-talk.voice.relay",
    "vibe-talk.voice.read-audio-source",
    "vibe-talk.voice.read-speed",
    "vibe-talk.voice.prompts",
    "vibe-talk.voice.mic",
    // Messages: own messages, empty, combining, rich headers, collapsed lines (moved
    // here from Screen and keys), and the link kinds the search bar can hide.
    "vibe-talk.voice.read-mode",
    "vibe-talk.voice.mark-own-read",
    "vibe-talk.voice.hide-empty",
    "vibe-talk.voice.combine",
    "vibe-talk.voice.rich-headers",
    "vibe-talk.voice.collapsed-lines",
    "vibe-talk.voice.hidden-link-kinds",
    // Threads view: pack/unpack, each thread's own choice, main rows, order, card fold.
    "vibe-talk.voice.threads-unpacked",
    "vibe-talk.voice.thread-card-states",
    "vibe-talk.voice.threads-main",
    "vibe-talk.voice.thread-order",
    "vibe-talk.voice.thread-card-folded",
    // Thread card lines: the most lines a card shows of its root, past which the root is
    // split or summarised (`#516 thread-card-max-lines`). The one value the server reads
    // itself ([`crate::threads::card_max_lines`]): it decides when a thread is named.
    "vibe-talk.voice.thread-card-max-lines",
    // Channels: who is who, the owner's choice of which account is him, the coding agent,
    // a person or another bot. `#488 slack-app-agents`.
    "vibe-talk.voice.identities",
];

/// The largest value one synced setting may hold, in bytes.
///
/// Generous against what the page actually writes — the largest is the thread-card
/// states, a few hundred short pairs as JSON — and small against anything else: a
/// setting is a choice, never message text, and a bound is what keeps it that way.
pub const MAX_VALUE_BYTES: usize = 64 * 1024;

/// The most entries one write may carry: every synced key at once, with room to grow.
pub const MAX_ENTRIES: usize = 64;

/// One synced setting as the server holds it: the key, its value (`None` in a write
/// means "forget it", the owner's choice returning to the shipped default), and when
/// the server took it, on its own clock — the stamp per-key last-writer-wins compares.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SyncedSetting {
    /// The `localStorage` key, one of [`SYNCED_KEYS`].
    pub key: String,
    /// The exact string the page keeps under that key, or `None` for no value held.
    pub value: Option<String>,
    /// When the server accepted this value, milliseconds since the Unix epoch.
    pub updated_ms: i64,
}

/// Why a write of synced settings was refused, in words the caller can be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A key that is not one of [`SYNCED_KEYS`].
    UnknownKey(String),
    /// A value over [`MAX_VALUE_BYTES`].
    ValueTooLarge(String),
    /// More than [`MAX_ENTRIES`] entries in one write.
    TooManyEntries,
    /// The same key twice in one write, which would make "last" mean two things.
    DuplicateKey(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::UnknownKey(key) => {
                write!(f, "{key} is not a setting this server keeps")
            }
            Refusal::ValueTooLarge(key) => {
                write!(f, "the value for {key} is larger than a setting may be")
            }
            Refusal::TooManyEntries => write!(f, "too many settings in one write"),
            Refusal::DuplicateKey(key) => write!(f, "{key} appears twice in one write"),
        }
    }
}

/// Check one write's entries before any of them is stored, so a refused write changes
/// nothing at all.
///
/// # Errors
///
/// The first [`Refusal`] the entries earn.
pub fn validate(entries: &[(String, Option<String>)]) -> Result<(), Refusal> {
    if entries.len() > MAX_ENTRIES {
        return Err(Refusal::TooManyEntries);
    }
    let mut seen = std::collections::BTreeSet::new();
    for (key, value) in entries {
        if !SYNCED_KEYS.contains(&key.as_str()) {
            return Err(Refusal::UnknownKey(key.clone()));
        }
        if !seen.insert(key.as_str()) {
            return Err(Refusal::DuplicateKey(key.clone()));
        }
        if value.as_ref().is_some_and(|v| v.len() > MAX_VALUE_BYTES) {
            return Err(Refusal::ValueTooLarge(key.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_and_the_server_name_the_same_synced_keys() {
        let page = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/web/voice.js"))
            .expect("the page script");
        let start = page
            .find("const SYNCED_SETTING_KEYS = [")
            .expect("the page declares its synced keys");
        let rest = &page[start..];
        let end = rest.find("];").expect("the declaration ends");
        let block = &rest[..end];
        let mut page_keys: Vec<&str> = SYNCED_KEYS
            .iter()
            .copied()
            .filter(|key| block.contains(&format!("\"{key}\"")))
            .collect();
        page_keys.sort_unstable();
        let mut server_keys: Vec<&str> = SYNCED_KEYS.to_vec();
        server_keys.sort_unstable();
        assert_eq!(page_keys, server_keys, "every server key is on the page");
        // And nothing more: every quoted key-shaped string in the block is a server key.
        for quoted in block.split('"').skip(1).step_by(2) {
            assert!(
                SYNCED_KEYS.contains(&quoted),
                "the page syncs {quoted}, which the server would refuse"
            );
        }
    }

    #[test]
    fn screen_and_keys_keys_are_not_synced() {
        for local in [
            "vibe-talk.voice.msg-scale",
            "vibe-talk.voice.width",
            "vibe-talk.voice.bar-placement",
            "vibe-talk.voice.enter-sends",
            "vibe-talk.voice.single-keys",
            "vibe-talk.voice.composer-grow",
            "vibe-talk.token",
        ] {
            assert!(
                !SYNCED_KEYS.contains(&local),
                "{local} is per-device and must stay off the server"
            );
        }
    }

    #[test]
    fn validation_refuses_what_the_server_does_not_keep() {
        let own = SYNCED_KEYS[0].to_owned();
        assert!(validate(&[(own.clone(), Some("on".into()))]).is_ok());
        assert_eq!(
            validate(&[("vibe-talk.voice.msg-scale".into(), Some("2".into()))]),
            Err(Refusal::UnknownKey("vibe-talk.voice.msg-scale".into()))
        );
        assert_eq!(
            validate(&[(own.clone(), Some("x".repeat(MAX_VALUE_BYTES + 1)))]),
            Err(Refusal::ValueTooLarge(own.clone()))
        );
        assert_eq!(
            validate(&[(own.clone(), None), (own.clone(), None)]),
            Err(Refusal::DuplicateKey(own))
        );
    }
}
