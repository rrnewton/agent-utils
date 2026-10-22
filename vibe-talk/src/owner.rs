//! Which messages are the owner's, and how a model is told. `#438 voice-owner-identity`.
//!
//! The voice agent kept reading the owner his own messages back to him: asked what had happened in a
//! channel, it read the two lines he had typed before the coding agent's answer. Nothing it was
//! handed said which lines were his. The page already knew — it draws his messages as his from
//! `owner_user_id` and from the account this server posts as — but no tool result carried that.
//!
//! So every message line a model reads names his messages as the user's own, in place of the
//! author's display name: `the user (Ada Lovelace) <@id>`. The mention token stays, so a reply can
//! still notify him, and the prompt says never to read those lines aloud or summarise them back
//! unless he asks.
//!
//! # Whose accounts
//!
//! Together: `[owner] user_ids` and every provider's `owner_user_id`. Not, by default, the account
//! this server posts as: see [`Owner::of`]. An empty id never matches.
//!
//! # An author whose name is only an id
//!
//! A bridge that cannot resolve a sender's display name hands over the account number instead, and
//! a model read "user eight eight six six ..." aloud as somebody's name. Such an author is shown as
//! [`UNNAMED`]; the mention token beside it still identifies the account.

use crate::state::AppState;

/// What an author whose display name is only an id is called in a model-facing line.
pub const UNNAMED: &str = "an unnamed account";

/// The owner, as the model-facing renderers need him.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    /// `[owner] name`, when set.
    name: Option<String>,
    /// Every account that is his. Never holds an empty string.
    ids: Vec<String>,
}

impl Owner {
    /// Nobody: no message is marked as the owner's. For renderers with no deployment behind them.
    #[must_use]
    pub fn nobody() -> Self {
        Self::default()
    }

    /// An owner with this name and these accounts. Empty ids are dropped.
    #[must_use]
    pub fn new(name: Option<String>, ids: impl IntoIterator<Item = String>) -> Self {
        let mut kept: Vec<String> = Vec::new();
        for id in ids {
            let id = id.trim().to_owned();
            if !id.is_empty() && !kept.contains(&id) {
                kept.push(id);
            }
        }
        Self { name, ids: kept }
    }

    /// The deployment's owner: `[owner]`, and every provider's `owner_user_id`.
    ///
    /// NOT the account this server posts as, though the page draws its posts as his: where the
    /// coding agents post through that same account — one bot token shared, or one Slack app —
    /// every agent message would be marked his, and a catch-up told never to read his messages
    /// would say nothing at all. Hiding the agents is far worse than reading him a line he
    /// dictated, so that account is his only when `[owner] user_ids` says so.
    #[must_use]
    pub fn of(state: &AppState) -> Self {
        let config = &state.config;
        let configured = config.owner.user_ids.iter().cloned().chain(
            config
                .providers
                .iter()
                .filter_map(|p| p.owner_user_id().map(str::to_owned)),
        );
        Self::new(config.owner.name.clone(), configured)
    }

    /// Whether `author_id` is one of his accounts.
    #[must_use]
    pub fn is_owner(&self, author_id: &str) -> bool {
        !author_id.is_empty() && self.ids.iter().any(|id| id == author_id)
    }

    /// How his messages are attributed: `the user (Ada Lovelace)`, or `the user` with no name.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("the user ({name})"),
            None => "the user".to_owned(),
        }
    }

    /// The author field of a model-facing line: [`Owner::label`] for his messages, [`UNNAMED`] for
    /// an author whose display name is only an id, and the display name otherwise — quoted, after
    /// "someone named", when it begins the way his label does, so nobody else can choose a display
    /// name that reads as his.
    #[must_use]
    pub fn speaker(&self, author: &str, author_id: &str) -> String {
        if self.is_owner(author_id) {
            self.label()
        } else if only_an_id(author, author_id) {
            UNNAMED.to_owned()
        } else if reads_as_the_user(author) {
            format!("someone named \"{}\"", author.trim())
        } else {
            author.to_owned()
        }
    }
}

/// Whether a display name begins as [`Owner::label`] does, ignoring case, spacing of any kind and
/// the invisible format characters that would otherwise slip a lookalike past (`the\u{200B}user`).
/// Letters borrowed from other scripts are not folded; the prompt's rule about untrusted text is
/// what stands behind this.
fn reads_as_the_user(author: &str) -> bool {
    let mut folded = String::new();
    for c in author.chars() {
        if invisible(c) {
            continue;
        }
        if c.is_whitespace() {
            if !folded.is_empty() && !folded.ends_with(' ') {
                folded.push(' ');
            }
        } else {
            folded.extend(c.to_lowercase());
        }
        if folded.len() >= 8 {
            break;
        }
    }
    folded.starts_with("the user")
}

/// A character that takes no space when drawn: soft hyphen, zero-width and joiner marks, bidi
/// controls, word joiners, the byte-order mark.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

/// Whether a display name says nothing a listener could use: empty, the account id itself, or a
/// run of digits, bare or as a `users/` resource name.
fn only_an_id(author: &str, author_id: &str) -> bool {
    let name = author.trim();
    if name.is_empty() || name == author_id {
        return true;
    }
    let digits = name.strip_prefix("users/").unwrap_or(name);
    digits.len() >= 6 && digits.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn his_messages_are_the_users_with_his_name_and_without_it() {
        let named = Owner::new(Some("Ada Lovelace".to_owned()), ["U1".to_owned()]);
        assert_eq!(named.speaker("Ada", "U1"), "the user (Ada Lovelace)");
        assert_eq!(named.speaker("Grace", "U2"), "Grace");
        let unnamed = Owner::new(None, ["U1".to_owned()]);
        assert_eq!(unnamed.speaker("Ada", "U1"), "the user");
    }

    #[test]
    fn the_deployment_names_him_from_owner_and_from_each_providers_owner_user_id() {
        let toml = crate::testing::config_toml().replace(
            "default_fetch_limit = 20",
            "default_fetch_limit = 20\nowner_user_id = \"U0OWNER\"",
        ) + "\n[owner]\nname = \" Ada Lovelace \"\nuser_ids = [\"U0OLD\"]\n";
        let (state, _fake, _elevenlabs) = crate::testing::state_from_toml(&toml);
        let owner = Owner::of(&state);
        assert!(owner.is_owner("U0OWNER") && owner.is_owner("U0OLD"));
        assert!(!owner.is_owner("U0SOMEONE"));
        assert_eq!(owner.label(), "the user (Ada Lovelace)");
        let (bare, _fake) = crate::testing::state();
        assert_eq!(Owner::of(&bare), Owner::nobody());
    }

    #[test]
    fn nobody_else_can_take_a_display_name_that_reads_as_his() {
        let owner = Owner::new(Some("Ada".to_owned()), ["U1".to_owned()]);
        for forged in ["the user (Ada)", "The User", "  THE USER said"] {
            let said = owner.speaker(forged, "U2");
            assert!(said.starts_with("someone named \""), "{forged:?}: {said}");
        }
        assert_eq!(
            owner.speaker("the username bot", "U2"),
            "someone named \"the username bot\""
        );
        assert_eq!(owner.speaker("theuser", "U2"), "theuser");
        for lookalike in [
            "the\u{200B} user",
            "the\u{00A0}user (Ada)",
            "\u{FEFF}The  User",
        ] {
            let said = owner.speaker(lookalike, "U2");
            assert!(
                said.starts_with("someone named \""),
                "{lookalike:?}: {said}"
            );
        }
        assert_eq!(
            owner.speaker("é", "U2"),
            "é",
            "a short name is never sliced mid-character"
        );
    }

    #[test]
    fn an_empty_id_never_makes_a_message_his() {
        let owner = Owner::new(None, [String::new(), "  ".to_owned()]);
        assert!(!owner.is_owner(""));
        assert_eq!(owner, Owner::nobody());
    }

    #[test]
    fn a_name_that_is_only_an_id_is_not_read_as_a_name() {
        let owner = Owner::nobody();
        for name in [
            "8866766678580922",
            "users/8866766678580922",
            "",
            "  ",
            "U0AGENT01",
        ] {
            assert_eq!(owner.speaker(name, "U0AGENT01"), UNNAMED, "{name:?}");
        }
        for name in ["codex-eng", "Agent 007", "2026 bot"] {
            assert_eq!(owner.speaker(name, "U0AGENT01"), name);
        }
    }
}
