//! The admission record for one Discord post, carried as the bridge event's provider payload.
//!
//! This is NOT an inter-team envelope and adds no relay mechanism. Relay traffic keeps using
//! `agent-teams`' signed `agent-teams-envelope` fenced block (`rs/agent-teams/src/relay.rs`),
//! whose ID, body hash and receiving ledger are authoritative for exactly-once delivery. This
//! record only says how this host's plugin admitted a post (origin, local target, Discord
//! identity), so the bridge and logs can see it.
//!
//! Its dedupe key names the receiving host as well as the Discord message, because each bot
//! handles a post addressed to both for itself: the two hosts' keys differ, and each host
//! delivers (and ACKs) at most once. Within a host the durable guarantee is the cursor.

use std::collections::{HashSet, VecDeque};

use chat_subscription::ProviderPayload;
use serde_json::{json, Map, Value};

use crate::addressing::Target;
use crate::ids::Snowflake;

/// Provider payload schema of [`Admission`].
pub const PAYLOAD_SCHEMA: &str = "discord.admission.v1";

/// Who wrote the post, as admitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A human addressing this host (sender authorization is checked separately).
    Owner {
        /// Discord user id.
        user: Snowflake,
    },
    /// A configured peer bot; the post is a relay envelope candidate.
    PeerBot {
        /// Discord user id of the peer bot.
        bot: Snowflake,
        /// The team that bot speaks for (`FLEET@HOST`).
        team: String,
    },
}

/// One admitted post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admission {
    /// Who wrote it.
    pub origin: Origin,
    /// The receiving host (this host).
    pub host: String,
    /// The local target; `None` for relay posts, whose target is inside the
    /// `agent-teams-envelope` block and is checked by `agent-teams relay-in`.
    pub target: Option<Target>,
    /// Discord channel.
    pub channel: Snowflake,
    /// Discord message.
    pub message: Snowflake,
}

impl Admission {
    /// The dedupe key: `discord:<channel>:<message>@<host>`.
    #[must_use]
    pub fn dedupe_key(&self) -> String {
        dedupe_key(self.channel, self.message, &self.host)
    }

    /// The JSON form carried as the bridge event's provider payload.
    #[must_use]
    pub fn to_json(&self) -> Map<String, Value> {
        let origin = match &self.origin {
            Origin::Owner { user } => json!({"kind": "owner", "user": user.to_string()}),
            Origin::PeerBot { bot, team } => {
                json!({"kind": "peer_bot", "bot": bot.to_string(), "team": team})
            }
        };
        let target = match &self.target {
            None => Value::Null,
            Some(Target::Bridge) => json!({"kind": "bridge", "host": self.host}),
            Some(Target::Agent { name }) => {
                json!({"kind": "agent", "agent": name, "host": self.host})
            }
        };
        let mut map = Map::new();
        map.insert("origin".to_owned(), origin);
        map.insert("target".to_owned(), target);
        map.insert("host".to_owned(), json!(self.host));
        map.insert("channel".to_owned(), json!(self.channel.to_string()));
        map.insert("message".to_owned(), json!(self.message.to_string()));
        map.insert("dedupe_key".to_owned(), json!(self.dedupe_key()));
        map
    }

    /// The provider payload for the bridge event.
    ///
    /// # Errors
    ///
    /// Returns the validation message when the payload exceeds the protocol's bounds.
    pub fn to_payload(&self) -> Result<ProviderPayload, String> {
        ProviderPayload::new(PAYLOAD_SCHEMA, self.to_json()).map_err(|error| error.to_string())
    }
}

/// The dedupe key for one Discord message as handled by one host.
#[must_use]
pub fn dedupe_key(channel: Snowflake, message: Snowflake, host: &str) -> String {
    format!("discord:{channel}:{message}@{host}")
}

/// A bounded set of recently handled dedupe keys. The durable guarantee is the cursor (a
/// message at or below the committed cursor is never listed again); this set absorbs overlap
/// within one subscription, such as a page boundary re-listing a message.
#[derive(Debug)]
pub struct DedupeSet {
    order: VecDeque<String>,
    members: HashSet<String>,
    capacity: usize,
}

impl DedupeSet {
    /// An empty set holding at most `capacity` keys (oldest evicted first).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::new(),
            members: HashSet::new(),
            capacity: capacity.max(1),
        }
    }

    /// Record `key`; returns `false` when it was already present.
    pub fn insert(&mut self, key: String) -> bool {
        if self.members.contains(&key) {
            return false;
        }
        if self.order.len() == self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.members.remove(&old);
            }
        }
        self.members.insert(key.clone());
        self.order.push_back(key);
        true
    }

    /// Whether `key` was recorded.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.members.contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(host: &str, target: Option<Target>) -> Admission {
        Admission {
            origin: Origin::Owner {
                user: Snowflake::from_u64(7),
            },
            host: host.to_owned(),
            target,
            channel: Snowflake::from_u64(1_566_504_459_499_217_097),
            message: Snowflake::from_u64(99),
        }
    }

    #[test]
    fn dedupe_keys_differ_per_host_so_each_bot_handles_its_own() {
        let ours = envelope("newton-fedora", Some(Target::Bridge));
        let theirs = envelope("lead-host", Some(Target::Bridge));
        assert_ne!(ours.dedupe_key(), theirs.dedupe_key());
        assert_eq!(
            ours.dedupe_key(),
            "discord:1566504459499217097:99@newton-fedora"
        );
    }

    #[test]
    fn payload_is_valid_and_carries_the_key() {
        let env = envelope("newton-fedora", None);
        let payload = env.to_payload().expect("valid payload");
        assert_eq!(payload.schema(), PAYLOAD_SCHEMA);
        assert_eq!(payload.data()["dedupe_key"], json!(env.dedupe_key()));
        assert_eq!(payload.data()["target"], Value::Null);
    }

    #[test]
    fn dedupe_set_rejects_repeats_and_evicts_oldest() {
        let mut set = DedupeSet::new(2);
        assert!(set.insert("a".to_owned()));
        assert!(!set.insert("a".to_owned()));
        assert!(set.insert("b".to_owned()));
        assert!(set.insert("c".to_owned()));
        assert!(!set.contains("a"));
        assert!(set.contains("c"));
    }
}
