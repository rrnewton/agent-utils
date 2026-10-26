//! The owner's own messages START read, as recorded state. `#549 own-read-stored`, after `#536
//! mark-own-messages-read`.
//!
//! # The rule
//!
//! There is one read bit per message, and it is this server's Done mark ([`StateStore::dismiss`]).
//! "Mark my own messages as read" decides only where that bit STARTS for a message of the owner's:
//!
//! 1. At the first sight of one of his messages here — pushed, polled, relisted, read for a page, or
//!    posted through this server — its bit is decided: Done if the setting is on, left unread if it
//!    is off. The owner, 2026-10-26: "it's just a question of whether the state starts as read or
//!    starts as unread when the message is ingested".
//! 2. Decided ONCE ([`StateStore::decide_own`] records it), never again: one he puts back stays
//!    unread across restarts, relists and re-reads, and Done, Undo and e work on his messages as on
//!    anyone's. Turning the setting on or off changes what happens to messages first seen from then
//!    on; "Apply to existing messages" ([`crate::ops::apply_own_read`]) marks his undone ones read
//!    once, on request.
//! 3. The marks it writes are held under the dismissal bound APART from the reader's own (see
//!    [`crate::store::Retention::max_dismissals`]), so a busy owner's own words never push out a
//!    mark he made himself.
//!
//! Every surface then reads the one bit: the to-do list, the page's counts, icons and thread cards,
//! and what an agent is told.
//!
//! # Where first sight is
//!
//! - **The live hub** notes every message it publishes for the first time ([`Sightings::note`],
//!   under its feed lock, never waiting), and [`run`] decides them in the background.
//! - **A write-scope read** of a window, a page or a timeline decides any message of his in it not
//!   decided yet ([`settle`]), after deciding whatever the hub has noted and not yet decided. So a
//!   message that arrived while this server was down is decided at the first read that meets it.
//! - **A post** through this server decides what it posted.
//!
//! A read-scope read writes nothing durable, here as everywhere: it is answered as if the messages
//! it meets that are not decided yet were decided now, and the next write-scope read or the hub
//! decides them for real.
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
//!
//! A message is decided the first time it is seen as HIS: one from an account he later calls "me"
//! is decided at the next sight after that.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::client_settings::SyncedSetting;
use crate::model::{ChannelId, Message, MessageId};
use crate::state::AppState;
use crate::store::{StateStore, StoreError};

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
    /// Whether his own messages start read.
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

    /// Whether `message` is one of his whose read state this rule decides: his, not read as noise
    /// already (which says so for itself), and with an id the store can order.
    fn decides(&self, accounts: &Accounts, sighting: &Sighting) -> bool {
        !sighting.noise
            && sighting.id.numeric().is_some()
            && self.owns(accounts, &sighting.author_id, sighting.posted_here)
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

/// One message as first seen: what deciding its read state asks of it.
#[derive(Clone, Debug)]
struct Sighting {
    id: MessageId,
    author_id: String,
    noise: bool,
    posted_here: bool,
}

impl Sighting {
    fn of(message: &Message, posted_here: bool) -> Self {
        Self {
            id: message.id.clone(),
            author_id: message.author_id.as_str().to_owned(),
            noise: message.noise,
            posted_here,
        }
    }
}

/// How many messages the live hub may have noted and not yet had decided. Past it the oldest is
/// let go; the next write-scope read that meets it decides it instead.
const MAX_WAITING: usize = 4_096;

/// The messages the live hub published for the first time, waiting for [`run`] to decide their
/// read state. `#549 own-read-stored`.
///
/// Noted under the hub's feed lock, so [`Sightings::note`] takes one brief lock of its own and never
/// waits; the store is written in the background.
#[derive(Debug, Default)]
pub struct Sightings {
    waiting: Mutex<VecDeque<(ChannelId, Sighting)>>,
    wake: tokio::sync::Notify,
}

impl Sightings {
    /// The hub published `message` to `channel` for the first time; `self_posted` is whether this
    /// server posted it.
    pub fn note(&self, channel: &ChannelId, message: &Message, self_posted: bool) {
        {
            let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            waiting.push_back((channel.clone(), Sighting::of(message, self_posted)));
            while waiting.len() > MAX_WAITING {
                waiting.pop_front();
            }
        }
        self.wake.notify_one();
    }

    fn take(&self) -> Vec<(ChannelId, Sighting)> {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }
}

/// Decide, in the background, the read state of every message of his the live hub publishes for
/// the first time. Spawned once at startup.
pub async fn run(state: AppState) {
    let sightings = std::sync::Arc::clone(state.live.own_sightings());
    loop {
        sightings.wake.notified().await;
        decide_waiting(&state).await;
    }
}

/// Decide whatever the live hub has noted and not yet had decided.
pub async fn decide_waiting(state: &AppState) {
    let waiting = state.live.own_sightings().take();
    if waiting.is_empty() {
        return;
    }
    let mut by_channel: BTreeMap<ChannelId, Vec<Sighting>> = BTreeMap::new();
    for (channel, sighting) in waiting {
        by_channel.entry(channel).or_default().push(sighting);
    }
    for (channel, sightings) in by_channel {
        decide(state, true, &channel, &sightings).await;
    }
}

/// The messages among `messages`, read for `caller` in `channel`, that are Done in this answer
/// because their first read state was decided now: the owner's own, first seen here, while "Mark my
/// own messages as read" is on. A caller adds them to the store's Done marks it read, before or
/// after this, so a read is never answered without the marks its own first sight wrote.
///
/// With write scope the decision is recorded, once ([`StateStore::decide_own`]); with read scope
/// nothing is written, and the messages not yet decided are answered as if decided now. Whatever
/// the live hub has noted is decided first, so a read never answers ahead of a message the hub saw.
pub async fn settle(
    state: &AppState,
    caller: crate::auth::Scope,
    channel: &ChannelId,
    messages: &[Message],
) -> HashSet<MessageId> {
    decide_waiting(state).await;
    let sightings: Vec<Sighting> = messages
        .iter()
        .map(|message| Sighting::of(message, state.live.is_self_posted(&message.id)))
        .collect();
    decide(
        state,
        caller >= crate::auth::Scope::Write,
        channel,
        &sightings,
    )
    .await
}

/// Decide what this server just posted to `channel`.
pub async fn settle_posted(state: &AppState, channel: &ChannelId, posted: &[Message]) {
    let sightings: Vec<Sighting> = posted
        .iter()
        .map(|message| Sighting::of(message, true))
        .collect();
    decide(state, true, channel, &sightings).await;
}

/// Decide the first read state of each of `sightings` that is his and not decided yet, recording it
/// when `write`, and answer those that are Done because of it. A store that cannot answer decides
/// nothing: a server with no store has no Done at all.
async fn decide(
    state: &AppState,
    write: bool,
    channel: &ChannelId,
    sightings: &[Sighting],
) -> HashSet<MessageId> {
    if sightings.is_empty() {
        return HashSet::new();
    }
    let choice = state.own_read.choice(state.store.as_ref()).await;
    let accounts = accounts_of(state);
    let mut his: Vec<MessageId> = sightings
        .iter()
        .filter(|sighting| choice.decides(&accounts, sighting))
        .map(|sighting| sighting.id.clone())
        .collect();
    his.sort();
    his.dedup();
    if his.is_empty() {
        return HashSet::new();
    }
    let decided: HashSet<MessageId> = match state.store.own_decided(channel, &his).await {
        Ok(decided) => decided.into_iter().collect(),
        Err(error) => {
            note_failure(
                channel,
                &error,
                "read which of the owner's messages were decided",
            );
            return HashSet::new();
        }
    };
    let undecided: Vec<MessageId> = his.into_iter().filter(|id| !decided.contains(id)).collect();
    if undecided.is_empty() {
        return HashSet::new();
    }
    if !write {
        return if choice.on {
            undecided.into_iter().collect()
        } else {
            HashSet::new()
        };
    }
    match state.store.decide_own(channel, &undecided, choice.on).await {
        Ok(fresh) if choice.on => fresh.into_iter().collect(),
        Ok(_) => HashSet::new(),
        Err(error) => {
            note_failure(
                channel,
                &error,
                "record the first read state of the owner's messages",
            );
            HashSet::new()
        }
    }
}

/// Log a store failure, but not the absence of a store, which is a deployment's choice.
fn note_failure(channel: &ChannelId, error: &StoreError, what: &str) {
    if !matches!(error, StoreError::Unavailable(_)) {
        tracing::warn!(channel = %channel, %error, "could not {what}");
    }
}

/// The configured accounts, and the accounts this server posts as.
fn accounts_of(state: &AppState) -> Accounts {
    let selves = state
        .providers
        .entries()
        .iter()
        .filter_map(|entry| entry.client.self_author_id())
        .chain(state.chat.self_author_id());
    Accounts::of(&state.config, selves)
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
