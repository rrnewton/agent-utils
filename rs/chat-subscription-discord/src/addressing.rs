//! The addressing rule: which Discord posts this host acts on.
//!
//! Several hosts' bots share one channel, so "I can see it" never means "it is for me". The
//! rule, validated by the owner after a bot ACKed a message meant for the other bot:
//!
//! * A post is for this host only when its **text** @-mentions this host's bot user id
//!   (`<@ID>` or `<@!ID>`, compared as parsed snowflakes) or names an alias this host owns as a
//!   whole `@alias` token. Never by substring: a longer id that merely contains ours, a bare id
//!   in prose, or the bot's display name in text is not a mention.
//! * Seeing a post, a reply-to (Discord adds the replied-to author to the API's `mentions`
//!   array when the reply pings), or the bot's name in the text does not address it. This
//!   module therefore reads only the text; it never consults the `mentions` array or the
//!   referenced message.
//! * A post addressing both bots is handled by each bot for itself.
//! * A post addressed only to another bot is ignored: no ACK, no rejection, nothing.
//! * Ambiguous posts get no action.
//! * One agent is targeted with `<@BOT> agent=NAME@HOST: request`. A target on another host
//!   is rejected when only this bot was addressed, and ignored when the other host's bot was
//!   addressed too (that bot owns it). An unknown local agent is rejected.
//! * Peer-bot posts are relay envelopes, never owner prompts, whatever their text says.
//!   This host's own posts are never acted on, so two bots cannot ACK each other in a loop.
//!
//! Sender authorization (is this human an owner?) is a separate, later check in
//! [`crate::plugin`] and in the bridge itself.

use std::collections::{BTreeMap, BTreeSet};

use crate::ids::Snowflake;

/// This host's addressing identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressingPolicy {
    /// This host's bot user id.
    pub own_bot: Snowflake,
    /// This host's name, as written in `agent=NAME@HOST`.
    pub host: String,
    /// Peer bots on other hosts, mapped to the team they speak for (`FLEET@HOST`).
    pub peer_bots: BTreeMap<Snowflake, String>,
    /// Aliases this host owns, matched as whole `@alias` tokens, ASCII case-insensitively.
    pub aliases: BTreeSet<String>,
    /// When set, the agents an `agent=NAME@HOST` target may name on this host. `None` defers
    /// the unknown-agent check to the bridge (its `!@NAME` handling answers unknown names).
    pub local_agents: Option<BTreeSet<String>>,
}

/// The facts about one post the rule may use. Deliberately excludes the API `mentions` array
/// and the referenced message: neither addresses a post.
#[derive(Clone, Copy, Debug)]
pub struct Post<'a> {
    /// Authenticated author id.
    pub author: Snowflake,
    /// Whether Discord marks the author as a bot account.
    pub author_is_bot: bool,
    /// The post's text.
    pub content: &'a str,
}

/// Who a delivered request is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The bridge's own target (normally the host's coordinator).
    Bridge,
    /// One named agent on this host.
    Agent {
        /// Agent name.
        name: String,
    },
}

/// How the post addressed this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressedVia {
    /// `<@OWN_BOT_ID>` in the text.
    Mention,
    /// `@alias` in the text.
    Alias(String),
}

/// A request this host should deliver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Addressed {
    /// Who receives it.
    pub target: Target,
    /// The request text with the leading address header removed.
    pub request: String,
    /// How the post addressed this host.
    pub via: AddressedVia,
    /// Whether another known bot was also addressed (each bot handles it for itself).
    pub also_addressed_peer: bool,
}

/// Why a post is ignored. Ignored posts draw no reaction and no reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IgnoreReason {
    /// This host's own bot wrote it.
    OwnPost,
    /// Neither this bot's id nor one of this host's aliases is in the text.
    NotAddressed,
    /// Addressed to several bots, and the `agent=` target names another host.
    TargetOnOtherHost,
    /// A bot that is neither this host's nor a configured peer.
    UnknownBot,
    /// Addressed, but what to do is not determined.
    Ambiguous(&'static str),
}

/// Why an addressed post is refused. The caller may tell the owner; it never delivers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// `agent=` names a host that is not this one, and only this bot was addressed.
    WrongHost {
        /// The host the post named.
        host: String,
    },
    /// `agent=` names an agent this host does not have.
    UnknownAgent {
        /// The agent the post named.
        name: String,
    },
    /// `agent=` is present but not `NAME@HOST`.
    MalformedTarget,
}

/// The rule's verdict for one post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Take no action of any kind.
    Ignore(IgnoreReason),
    /// Addressed to this host but refused.
    Reject(RejectReason),
    /// A configured peer bot's post: a relay envelope candidate for `agent-teams relay-in`,
    /// never a prompt.
    Relay {
        /// The team the peer bot speaks for.
        peer_team: String,
    },
    /// Deliver to a local target.
    Deliver(Addressed),
}

impl AddressingPolicy {
    /// Apply the addressing rule to one post.
    #[must_use]
    pub fn classify(&self, post: &Post<'_>) -> Verdict {
        if post.author == self.own_bot {
            return Verdict::Ignore(IgnoreReason::OwnPost);
        }
        if let Some(team) = self.peer_bots.get(&post.author) {
            return Verdict::Relay {
                peer_team: team.clone(),
            };
        }
        if post.author_is_bot {
            return Verdict::Ignore(IgnoreReason::UnknownBot);
        }
        let mentions = mention_ids(post.content);
        let mentions_us = mentions.contains(&self.own_bot);
        let mentions_peer = mentions.iter().any(|id| self.peer_bots.contains_key(id));
        let alias = self.alias_in(post.content);
        let via = if mentions_us {
            AddressedVia::Mention
        } else if let Some(alias) = alias {
            AddressedVia::Alias(alias)
        } else {
            return Verdict::Ignore(IgnoreReason::NotAddressed);
        };
        let body = self.strip_header(post.content);
        let (specs, request) = match split_agent_specs(body) {
            Ok(split) => split,
            Err(()) => return Verdict::Reject(RejectReason::MalformedTarget),
        };
        let request = request.trim().to_owned();
        if request.is_empty() {
            return Verdict::Ignore(IgnoreReason::Ambiguous("empty request"));
        }
        let (local, remote): (Vec<_>, Vec<_>) = specs
            .into_iter()
            .partition(|(_, host)| host.eq_ignore_ascii_case(&self.host));
        let target = match (local.len(), remote.first()) {
            (0, None) => Target::Bridge,
            (0, Some((_, host))) => {
                return if mentions_peer {
                    Verdict::Ignore(IgnoreReason::TargetOnOtherHost)
                } else {
                    Verdict::Reject(RejectReason::WrongHost { host: host.clone() })
                };
            }
            (1, _) => {
                let name = local[0].0.clone();
                if let Some(known) = &self.local_agents {
                    if !known.contains(&name) {
                        return Verdict::Reject(RejectReason::UnknownAgent { name });
                    }
                }
                Target::Agent { name }
            }
            _ => return Verdict::Ignore(IgnoreReason::Ambiguous("several local agents named")),
        };
        Verdict::Deliver(Addressed {
            target,
            request,
            via,
            also_addressed_peer: mentions_peer,
        })
    }

    fn alias_in(&self, content: &str) -> Option<String> {
        content
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')'))
            .filter_map(|token| token.strip_prefix('@'))
            .map(|token| token.trim_end_matches(['.', '!', '?']))
            .find_map(|token| {
                self.aliases
                    .iter()
                    .find(|alias| alias.eq_ignore_ascii_case(token))
                    .cloned()
            })
    }

    /// Drop the leading run of mentions and alias tokens (and separating `,`/`:`).
    fn strip_header<'a>(&self, content: &'a str) -> &'a str {
        let mut rest = content.trim_start();
        loop {
            let before = rest;
            if let Some((_, after)) = parse_mention(rest) {
                rest = after;
            } else if let Some(token) = rest.strip_prefix('@') {
                let end = token
                    .find(|c: char| c.is_whitespace() || matches!(c, ',' | ':' | ';'))
                    .unwrap_or(token.len());
                if self
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(&token[..end]))
                {
                    rest = &token[end..];
                }
            }
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ':'));
            if rest.len() == before.len() {
                return rest;
            }
        }
    }
}

/// Every user mention in `content`, in order: `<@ID>` and the legacy nickname form `<@!ID>`.
/// Role (`<@&ID>`) and channel (`<#ID>`) mentions are not user mentions.
#[must_use]
pub fn mention_ids(content: &str) -> Vec<Snowflake> {
    let mut found = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("<@") {
        match parse_mention(&rest[start..]) {
            Some((id, after)) => {
                found.push(id);
                rest = after;
            }
            None => rest = &rest[start + 2..],
        }
    }
    found
}

/// Parse one user mention at the very start of `text`.
fn parse_mention(text: &str) -> Option<(Snowflake, &str)> {
    let inner = text.strip_prefix("<@")?;
    let inner = inner.strip_prefix('!').unwrap_or(inner);
    let close = inner.find('>')?;
    let id = Snowflake::parse(&inner[..close])?;
    Some((id, &inner[close + 1..]))
}

/// Split leading `agent=NAME@HOST` specs (space or comma separated, ended by `:`) from the
/// request. No `agent=` at the head means no specs. `Err` means a malformed spec.
#[allow(clippy::type_complexity)]
fn split_agent_specs(body: &str) -> Result<(Vec<(String, String)>, &str), ()> {
    if !body.starts_with("agent=") {
        return Ok((Vec::new(), body));
    }
    let (header, request) = body.split_once(':').ok_or(())?;
    let mut specs = Vec::new();
    for token in header.split(|c: char| c.is_whitespace() || c == ',') {
        if token.is_empty() {
            continue;
        }
        let spec = token.strip_prefix("agent=").ok_or(())?;
        let (name, host) = spec.split_once('@').ok_or(())?;
        if !valid_label(name) || !valid_label(host) {
            return Err(());
        }
        specs.push((name.to_owned(), host.to_owned()));
    }
    if specs.is_empty() {
        return Err(());
    }
    Ok((specs, request))
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 64
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DeepscryCoder2: this host's bot (newton-fedora).
    const OURS: u64 = 1_545_796_614_826_889_226;
    /// DeepScryCoder1: the other machine's bot.
    const OTHER: u64 = 1_532_416_065_114_607_829;
    const OWNER: u64 = 111_222_333_444_555_666;

    fn policy() -> AddressingPolicy {
        AddressingPolicy {
            own_bot: Snowflake::from_u64(OURS),
            host: "newton-fedora".to_owned(),
            peer_bots: BTreeMap::from([(
                Snowflake::from_u64(OTHER),
                "deepscry@lead-host".to_owned(),
            )]),
            aliases: BTreeSet::from(["newton-team".to_owned()]),
            local_agents: Some(BTreeSet::from(["coord".to_owned(), "lane-a".to_owned()])),
        }
    }

    fn owner(content: &str) -> Verdict {
        policy().classify(&Post {
            author: Snowflake::from_u64(OWNER),
            author_is_bot: false,
            content,
        })
    }

    fn deliver(verdict: Verdict) -> Addressed {
        match verdict {
            Verdict::Deliver(addressed) => addressed,
            other => panic!("expected Deliver, got {other:?}"),
        }
    }

    #[test]
    fn msg1_mentioning_only_the_other_bot_is_ignored() {
        // The incident: a message for DeepScryCoder1 was ACKed by DeepscryCoder2.
        assert_eq!(
            owner(&format!("<@{OTHER}> please rebase #3071 and report")),
            Verdict::Ignore(IgnoreReason::NotAddressed)
        );
        assert_eq!(
            owner(&format!("<@{OTHER}> agent=coord@lead-host: status?")),
            Verdict::Ignore(IgnoreReason::NotAddressed)
        );
        // Even when it names an agent on THIS host, only the other bot was addressed.
        assert_eq!(
            owner(&format!("<@{OTHER}> agent=coord@newton-fedora: status?")),
            Verdict::Ignore(IgnoreReason::NotAddressed)
        );
    }

    #[test]
    fn mention_of_this_bot_delivers_to_the_bridge() {
        let addressed = deliver(owner(&format!("<@{OURS}> what is the queue depth?")));
        assert_eq!(addressed.target, Target::Bridge);
        assert_eq!(addressed.request, "what is the queue depth?");
        assert_eq!(addressed.via, AddressedVia::Mention);
        assert!(!addressed.also_addressed_peer);
        // Legacy nickname mention form.
        let addressed = deliver(owner(&format!("<@!{OURS}>, ping")));
        assert_eq!(addressed.request, "ping");
    }

    #[test]
    fn mention_anywhere_in_text_addresses_but_header_is_only_stripped_at_the_head() {
        let addressed = deliver(owner(&format!("hey <@{OURS}> look at this")));
        assert_eq!(addressed.request, format!("hey <@{OURS}> look at this"));
    }

    #[test]
    fn substrings_names_and_bare_ids_do_not_address() {
        for content in [
            // Bot display name in text.
            "DeepscryCoder2 please look".to_owned(),
            "@DeepscryCoder2 please look".to_owned(),
            // Bare id in prose.
            format!("bot {OURS} is ours"),
            // Our id as a substring of a longer number inside mention syntax.
            format!("<@{OURS}1> hi"),
            format!("<@9{OURS}> hi"),
            // Role and channel mentions with our digits.
            format!("<@&{OURS}> hi"),
            format!("<#{OURS}> hi"),
            // Alias as a substring of a longer token.
            "@newton-team-b do it".to_owned(),
            "newton-team do it".to_owned(),
        ] {
            assert_eq!(
                owner(&content),
                Verdict::Ignore(IgnoreReason::NotAddressed),
                "{content}"
            );
        }
    }

    #[test]
    fn reply_to_and_api_mentions_do_not_address() {
        // A Discord reply to our bot's post puts our id in the API `mentions` array and the
        // referenced author, but the TEXT does not mention us. `Post` cannot even carry those
        // fields, so the only input is the text, which is unaddressed.
        assert_eq!(
            owner("thanks, that worked"),
            Verdict::Ignore(IgnoreReason::NotAddressed)
        );
    }

    #[test]
    fn alias_addresses_as_a_whole_token() {
        let addressed = deliver(owner("@newton-team: run the focused tests"));
        assert_eq!(addressed.via, AddressedVia::Alias("newton-team".to_owned()));
        assert_eq!(addressed.request, "run the focused tests");
        let addressed = deliver(owner("ok @Newton-Team, go"));
        assert_eq!(addressed.via, AddressedVia::Alias("newton-team".to_owned()));
    }

    #[test]
    fn both_bots_addressed_each_handles_for_itself() {
        let addressed = deliver(owner(&format!("<@{OTHER}> <@{OURS}> both: report status")));
        assert_eq!(addressed.target, Target::Bridge);
        assert_eq!(addressed.request, "both: report status");
        assert!(addressed.also_addressed_peer);
    }

    #[test]
    fn agent_target_on_this_host() {
        let addressed = deliver(owner(&format!(
            "<@{OURS}> agent=lane-a@newton-fedora: rebase"
        )));
        assert_eq!(
            addressed.target,
            Target::Agent {
                name: "lane-a".to_owned()
            }
        );
        assert_eq!(addressed.request, "rebase");
    }

    #[test]
    fn agent_target_unknown_agent_is_rejected() {
        assert_eq!(
            owner(&format!("<@{OURS}> agent=ghost@newton-fedora: hi")),
            Verdict::Reject(RejectReason::UnknownAgent {
                name: "ghost".to_owned()
            })
        );
        // Without a local agent list the bridge's `!@NAME` handling decides.
        let mut open = policy();
        open.local_agents = None;
        let verdict = open.classify(&Post {
            author: Snowflake::from_u64(OWNER),
            author_is_bot: false,
            content: &format!("<@{OURS}> agent=ghost@newton-fedora: hi"),
        });
        assert!(matches!(verdict, Verdict::Deliver(_)), "{verdict:?}");
    }

    #[test]
    fn agent_target_on_other_host() {
        // Only this bot addressed, but the target is elsewhere: refuse.
        assert_eq!(
            owner(&format!("<@{OURS}> agent=coord@lead-host: hi")),
            Verdict::Reject(RejectReason::WrongHost {
                host: "lead-host".to_owned()
            })
        );
        // Both bots addressed and the target is the other host's: that bot owns it.
        assert_eq!(
            owner(&format!("<@{OURS}> <@{OTHER}> agent=coord@lead-host: hi")),
            Verdict::Ignore(IgnoreReason::TargetOnOtherHost)
        );
    }

    #[test]
    fn two_targets_one_per_host_each_bot_takes_its_own() {
        let addressed = deliver(owner(&format!(
            "<@{OURS}> <@{OTHER}> agent=coord@lead-host agent=coord@newton-fedora: sync up"
        )));
        assert_eq!(
            addressed.target,
            Target::Agent {
                name: "coord".to_owned()
            }
        );
        assert_eq!(addressed.request, "sync up");
    }

    #[test]
    fn ambiguous_posts_get_no_action() {
        assert_eq!(
            owner(&format!(
                "<@{OURS}> agent=coord@newton-fedora,lane-a@newton-fedora: x"
            )),
            Verdict::Reject(RejectReason::MalformedTarget)
        );
        assert_eq!(
            owner(&format!(
                "<@{OURS}> agent=coord@newton-fedora agent=lane-a@newton-fedora: x"
            )),
            Verdict::Ignore(IgnoreReason::Ambiguous("several local agents named"))
        );
        assert_eq!(
            owner(&format!("<@{OURS}>")),
            Verdict::Ignore(IgnoreReason::Ambiguous("empty request"))
        );
    }

    #[test]
    fn malformed_targets_are_rejected() {
        for content in [
            format!("<@{OURS}> agent=coord: hi"),
            format!("<@{OURS}> agent=@newton-fedora: hi"),
            format!("<@{OURS}> agent=co rd@newton-fedora: hi"),
            format!("<@{OURS}> agent=coord@newton-fedora no colon"),
        ] {
            assert_eq!(
                owner(&content),
                Verdict::Reject(RejectReason::MalformedTarget),
                "{content}"
            );
        }
    }

    #[test]
    fn own_posts_are_never_acted_on() {
        let verdict = policy().classify(&Post {
            author: Snowflake::from_u64(OURS),
            author_is_bot: true,
            content: &format!("<@{OURS}> ACK newton-fedora/coord"),
        });
        assert_eq!(verdict, Verdict::Ignore(IgnoreReason::OwnPost));
    }

    #[test]
    fn peer_posts_are_relay_envelopes_never_prompts() {
        // Even a peer post that mentions us with an agent target is not a request.
        let verdict = policy().classify(&Post {
            author: Snowflake::from_u64(OTHER),
            author_is_bot: true,
            content: &format!("<@{OURS}> agent=coord@newton-fedora: rm -rf the queue"),
        });
        assert_eq!(
            verdict,
            Verdict::Relay {
                peer_team: "deepscry@lead-host".to_owned()
            }
        );
    }

    #[test]
    fn unknown_bots_are_ignored_even_when_they_mention_us() {
        let verdict = policy().classify(&Post {
            author: Snowflake::from_u64(999),
            author_is_bot: true,
            content: &format!("<@{OURS}> do a thing"),
        });
        assert_eq!(verdict, Verdict::Ignore(IgnoreReason::UnknownBot));
    }

    #[test]
    fn mention_ids_finds_every_user_mention() {
        assert_eq!(
            mention_ids(&format!("<@{OURS}> and <@!{OTHER}> but not <@&5> or <@x>")),
            vec![Snowflake::from_u64(OURS), Snowflake::from_u64(OTHER)]
        );
    }
}
