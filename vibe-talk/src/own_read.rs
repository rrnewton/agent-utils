//! The owner's own messages are read. `#536 mark-own-messages-read`.
//!
//! # What was wrong
//!
//! Settings' "Mark my own messages as read" was a rule of the page alone, and only half of one: a
//! row whose author was the owner was greyed and left out of Hide read, but its Done button still
//! offered Done — the read look over an unread control, the mismatch `#256 reply-marks-read` was
//! filed for — and this server's to-do list counted it as waiting for him. The owner, 2026-10-26:
//! "They are rendering as MY messages... but they are not marked read."
//!
//! # The rule
//!
//! While the setting is on — and it is on unless he turned it off: the page's own default — a
//! message from one of his accounts is read. Like a placeholder the noise rules catch (`#196
//! auto-read-noise`) it is JUDGED on every read and never written down: nothing is recorded per
//! message, so nothing crowds the reader's own Done marks out of the store's bound on them, and
//! turning the setting off brings every one of them straight back. ONE rule, asked in two places
//! from the same accounts: the page draws them read, offers no Done on them and counts them read in
//! every tally (`markOwnRead` in web/voice.js), and here the to-do list leaves them out
//! ([`crate::ops::todo`]) and "Done through here" writes no mark for one
//! ([`crate::ops::declare_bankruptcy`]). They are deliberately NOT added to a read's `dismissed`:
//! the page holds that answer, and one that said a message of his was read after he turned the
//! setting off would keep it read until the next read.
//!
//! # Whose messages
//!
//! The accounts the page draws as his, by the page's precedence ([`Choice::owns`]):
//!
//! 1. the synced who-is-who choice (`vibe-talk.voice.identities`): an account he called "me" is his,
//!    and one he called anything else is not, whatever else says so;
//! 2. `[owner] user_ids` and every provider's `owner_user_id` ([`crate::owner::Owner::of`]), which
//!    the page is told too (`owner_author_ids` in the client configuration);
//! 3. the account this server posts as, and any message this server posted — his words, dictated or
//!    typed here — unless the configuration names that account as a coding agent
//!    (`coding_agent_user_ids`). The page draws such an account as his regardless; here, where the
//!    answer decides what an agent is told is waiting, agents posting through one shared account
//!    are not taken to be him.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::client_settings::SyncedSetting;
use crate::model::Message;
use crate::store::StateStore;

/// The synced setting this follows: `"0"` is off, anything else or nothing at all is on.
pub const MARK_OWN_KEY: &str = "vibe-talk.voice.mark-own-read";

/// The synced who-is-who choice: a JSON object of account id to `"me"`, `"coder"`, `"human"` or
/// `"bot"`. `#488 slack-app-agents`.
pub const IDENTITY_KEY: &str = "vibe-talk.voice.identities";

/// The bucket of the who-is-who choice that is the owner.
const ME: &str = "me";

/// The owner's standing choices, as the synced settings hold them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// Whether his own messages are read.
    pub on: bool,
    /// Who is who: account id to bucket.
    pub identities: BTreeMap<String, String>,
}

impl Default for Choice {
    /// No setting held: on, as the page's default is, and nobody named.
    fn default() -> Self {
        Self {
            on: true,
            identities: BTreeMap::new(),
        }
    }
}

impl Choice {
    /// The choice the whole settings document holds.
    #[must_use]
    pub fn from_settings(settings: &[SyncedSetting]) -> Self {
        let mut choice = Self::default();
        for setting in settings {
            match setting.key.as_str() {
                MARK_OWN_KEY => choice.on = setting.value.as_deref() != Some("0"),
                IDENTITY_KEY => choice.identities = identities(setting.value.as_deref()),
                _ => {}
            }
        }
        choice
    }

    /// Whether `author_id` is one of his accounts (see the module documentation for the order).
    /// `posted_here` is whether this server posted the message.
    #[must_use]
    pub fn owns(&self, accounts: &Accounts, author_id: &str, posted_here: bool) -> bool {
        if author_id.is_empty() {
            return false;
        }
        if let Some(bucket) = self.identities.get(author_id) {
            return bucket == ME;
        }
        if accounts.owners.contains(author_id) {
            return true;
        }
        (posted_here || accounts.selves.contains(author_id)) && !accounts.agents.contains(author_id)
    }

    /// Whether `message` is read by this rule: on, his, and not already read as noise (which says
    /// so for itself, and is counted as noise).
    #[must_use]
    pub fn reads(&self, accounts: &Accounts, message: &Message, posted_here: bool) -> bool {
        self.on && !message.noise && self.owns(accounts, message.author_id.as_str(), posted_here)
    }
}

/// The who-is-who choice as stored; a value that is not an object of strings is no choice at all,
/// as it is to the page.
fn identities(value: Option<&str>) -> BTreeMap<String, String> {
    value
        .and_then(|raw| serde_json::from_str::<BTreeMap<String, serde_json::Value>>(raw).ok())
        .map(|held| {
            held.into_iter()
                .filter_map(|(id, bucket)| bucket.as_str().map(|bucket| (id, bucket.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// The accounts the configuration and the providers say things about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Accounts {
    /// `[owner] user_ids` and every provider's `owner_user_id`.
    pub owners: HashSet<String>,
    /// Every provider's `coding_agent_user_ids`.
    pub agents: HashSet<String>,
    /// The accounts this server posts as, where known.
    pub selves: HashSet<String>,
}

impl Accounts {
    /// The configured accounts of `config`, and `selves`, the accounts this server posts as.
    #[must_use]
    pub fn of(config: &crate::config::Config, selves: impl IntoIterator<Item = String>) -> Self {
        let agents = config
            .providers
            .iter()
            .flat_map(|p| p.coding_agent_user_ids.iter())
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
            .collect();
        Self {
            owners: owner_ids(config).into_iter().collect(),
            agents,
            selves: selves.into_iter().filter(|id| !id.is_empty()).collect(),
        }
    }
}

/// The owner's configured accounts, `[owner] user_ids` first and then each provider's
/// `owner_user_id`, each once, in that order. What the page is told as `owner_author_ids`.
#[must_use]
pub fn owner_ids(config: &crate::config::Config) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in config
        .owner
        .user_ids
        .iter()
        .map(String::as_str)
        .chain(config.providers.iter().filter_map(|p| p.owner_user_id()))
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if !ids.iter().any(|held| held == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

/// The owner's choice as last read from the store.
#[derive(Default)]
pub struct OwnRead {
    /// `None` until read, and again whenever the settings change or are purged: the next ask
    /// reads the store afresh. A generation, moved by every change, keeps a read that was on its
    /// way back when a change landed from filing its older answer.
    choice: Mutex<(Option<Choice>, u64)>,
    /// Set once by the first ask that finds no store at all, so such a server is not asked again.
    unavailable: OnceLock<()>,
}

impl OwnRead {
    /// The owner's synced settings changed, or were purged: forget the choice held, so the next
    /// ask reads it afresh.
    pub fn forget(&self) {
        let mut held = self
            .choice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held.0 = None;
        held.1 = held.1.wrapping_add(1);
    }

    /// The owner's choice: read from the store the first time it is wanted, and kept until the
    /// settings change. A store that cannot be read gives the default — on, nobody named — for this
    /// ask, which is what a page with nothing synced does, and is asked again next time.
    pub async fn choice(&self, store: &dyn StateStore) -> Choice {
        let generation = {
            let held = self
                .choice
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(choice) = &held.0 {
                return choice.clone();
            }
            held.1
        };
        if self.unavailable.get().is_some() {
            return Choice::default();
        }
        let choice = match store.client_settings().await {
            Ok(settings) => Choice::from_settings(&settings),
            Err(crate::store::StoreError::Unavailable(_)) => {
                let _ = self.unavailable.set(());
                return Choice::default();
            }
            Err(_) => return Choice::default(),
        };
        let mut held = self
            .choice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Filed only if nothing changed while the store was read; otherwise the next ask reads again.
        if held.1 == generation {
            held.0 = Some(choice.clone());
        }
        choice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(key: &str, value: Option<&str>) -> SyncedSetting {
        SyncedSetting {
            key: key.to_owned(),
            value: value.map(str::to_owned),
            updated_ms: 1,
        }
    }

    fn accounts() -> Accounts {
        Accounts {
            owners: ["U0OWNER".to_owned()].into(),
            agents: ["U0APP".to_owned()].into(),
            selves: ["U0BRIDGE".to_owned(), "U0APP".to_owned()].into(),
        }
    }

    #[test]
    fn the_setting_is_on_unless_it_says_zero() {
        assert!(Choice::from_settings(&[]).on);
        assert!(Choice::from_settings(&[setting(MARK_OWN_KEY, Some("1"))]).on);
        assert!(Choice::from_settings(&[setting(MARK_OWN_KEY, None)]).on);
        assert!(!Choice::from_settings(&[setting(MARK_OWN_KEY, Some("0"))]).on);
    }

    #[test]
    fn whose_messages_follows_the_pages_precedence() {
        let choice = Choice::from_settings(&[setting(
            IDENTITY_KEY,
            Some(r#"{"U0PERSON":"me","U0OWNER":"human","U0BRIDGE":"coder","U0X":7}"#),
        )]);
        let accounts = accounts();
        // The reader's word first, either way.
        assert!(choice.owns(&accounts, "U0PERSON", false));
        assert!(!choice.owns(&accounts, "U0OWNER", false));
        assert!(!choice.owns(&accounts, "U0BRIDGE", true));
        let plain = Choice::default();
        // The configured owner, the account this server posts as, and what it posted.
        assert!(plain.owns(&accounts, "U0OWNER", false));
        assert!(plain.owns(&accounts, "U0BRIDGE", false));
        assert!(plain.owns(&accounts, "U0ELSE", true));
        // Never a named coding agent, though this server posts through the same account.
        assert!(!plain.owns(&accounts, "U0APP", true));
        assert!(!plain.owns(&accounts, "U0ELSE", false));
        assert!(!plain.owns(&accounts, "", true));
        // A bucket that is not a string is no choice.
        assert!(!choice.owns(&accounts, "U0X", false));
    }

    #[tokio::test]
    async fn a_change_of_the_settings_is_read_afresh() {
        let store = crate::store::fake::FakeStore::default();
        store
            .apply_client_settings(&[(MARK_OWN_KEY.to_owned(), Some("0".to_owned()))], &[])
            .await
            .expect("stored");
        let own = OwnRead::default();
        assert!(!own.choice(&store).await.on);
        // Turned on again: the held choice is forgotten, and the next ask reads the store.
        store
            .apply_client_settings(&[(MARK_OWN_KEY.to_owned(), Some("1".to_owned()))], &[])
            .await
            .expect("stored");
        own.forget();
        assert!(own.choice(&store).await.on);
    }
}
