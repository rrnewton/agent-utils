//! The startup channel reachability probe.
//!
//! # Why this exists
//!
//! Every configured channel is a claim: "this bot can read that channel." Nothing in the
//! configuration can check that claim, because it is not a property of the configuration — it is a
//! property of a Discord server the operator clicked through in a browser, possibly weeks ago. The
//! observed failure is exactly what you would expect: the server starts cleanly, reports its
//! channels, and the mistake — the bot was never added to the channel — only surfaces later, as an
//! empty digest, which reads like a bug in this code rather than a missing invite.
//!
//! So the server asks Discord, once, at startup: can I read each of these channels? A failure
//! there is a configuration error and is reported as one.
//!
//! # What it does, precisely
//!
//! For each configured channel it performs the **smallest possible read** —
//! [`ChatClient::fetch_recent`] with a limit of one — and classifies the answer. It **never
//! writes**, not even to a channel configured `rw`: a startup check that posted to the owner's
//! channel on every restart would be its own bug. That property is structural, not a convention:
//! nothing in this module can reach [`ChatClient::post_message`].
//!
//! # Why the causes are kept apart
//!
//! "Channel unreachable" is not an actionable message. The five things that actually go wrong have
//! five different fixes — one is an OAuth2 invite, one is a per-channel permission override, one is
//! a mistyped snowflake, one is a bad token, and one is a toggle in the developer portal that
//! produces no error at all. Collapsing them costs the operator the hour this probe exists to save.
//!
//! # This vocabulary is shared, not Discord's alone
//!
//! [`Diagnosis`] began as "what happened when we read a channel" and is now the vocabulary EVERY
//! configuration check in this server speaks — see [`crate::diagnostics`], which re-runs these
//! checks on demand and adds the ElevenLabs and storage ones. That is deliberate: a second
//! taxonomy would mean two lists of causes, two sets of remedies, and two chances for one of them
//! to say "unavailable" and stop there. The variants below that name no vendor
//! ([`Diagnosis::Confirmed`], [`Diagnosis::NotConfigured`], [`Diagnosis::TimedOut`]) exist for
//! exactly that reason.
//!
//! # Channel heads are checked, never trusted in place of a read
//!
//! A provider that reads channel heads natively (remote-ops-audit W12) can say what each channel's
//! newest message is in one call, but a head cannot stand in for this probe's read: it does not
//! show that this bot may read the channel's history, nor that messages arrive with their content.
//! So the probe still reads one message per channel, and the head read is checked beside it
//! ([`check_heads`]): one call per provider that reads heads natively, none for any other. What
//! that catches is a head read that does not work (a bridge configured with `head_reads` that does
//! not serve it, say), which the poller and a catch-up survive by reading every channel as before,
//! at the cost of a wasted call each time.
//!
//! # A rate limit is waited out, after the server is up
//!
//! A deploy starts the new process just after the old one spent the provider's read budget, so
//! the probe's first read is often refused with a rate limit that clears in seconds. That says
//! nothing about the configuration, and refusing to start on it only made the supervisor start
//! the server again, whose probe spent the budget again. So the server binds first and probes
//! in the background ([`startup_check_patiently`]), waits as the provider asks, and stops only on
//! a definite failure or when its wait budget runs out (`#718 vt-startup-probe-restart`). Until
//! then `/healthz` and `/readyz` say the probe is pending ([`StartupStatus`]).

use std::borrow::Cow;
use std::time::Duration;

use crate::chat::{ChatClient, ChatError};
use crate::model::ChannelInfo;

/// Environment variable that skips the startup probe entirely.
///
/// Intended for offline development against `--fake-discord`, and as an escape hatch if the probe
/// itself ever misjudges a live deployment. Skipping is logged loudly by the caller: an invisible
/// skip would reintroduce the silent failure this module exists to remove.
pub const ENV_SKIP_STARTUP_PROBE: &str = "VIBE_TALK_SKIP_STARTUP_PROBE";

/// How many messages the probe reads per channel. One is enough to prove readability.
const PROBE_LIMIT: u16 = 1;

/// Discord JSON error code for "Missing Access".
///
/// Discord returns this when the bot cannot see the channel **at all**. That covers two distinct
/// operator mistakes — the bot was never added to the server, or it is in the server but has no
/// `View Channel` on that channel — and Discord does not tell them apart here. The rendered
/// message therefore names both, in the order they are worth checking, rather than guessing.
const DISCORD_MISSING_ACCESS: u64 = 50_001;

/// Discord JSON error code for "Missing Permissions".
///
/// Distinct from [`DISCORD_MISSING_ACCESS`]: the bot *can* see the channel, so the invite and the
/// `View Channel` grant are both fine, and what is missing is the narrower permission this read
/// needs — `Read Message History`.
const DISCORD_MISSING_PERMISSIONS: u64 = 50_013;

/// Discord JSON error code for "Unknown Channel".
const DISCORD_UNKNOWN_CHANNEL: u64 = 10_003;

/// What the probe concluded about one channel.
///
/// Ordered roughly from "fine" to "nothing works".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnosis {
    /// The channel was read successfully.
    Readable {
        /// How many messages came back (0 or 1, given [`PROBE_LIMIT`]).
        messages: usize,
        /// Every message that came back had empty `content`.
        ///
        /// This is the fingerprint of a disabled **Message Content Intent**, and it is the only
        /// misconfiguration in this list that Discord reports as a *success*. It is a warning
        /// rather than a failure because a message can legitimately be attachment- or embed-only,
        /// and refusing to start on that would be a false alarm the operator cannot clear.
        suspect_message_content_intent: bool,
    },
    /// Discord rejected the bot token itself. Global, not per-channel.
    InvalidToken,
    /// The bot cannot see this channel at all: not in the server, or no `View Channel`.
    NoAccess,
    /// The bot can see the channel but lacks `Read Message History` on it.
    MissingReadHistory,
    /// Discord does not know this channel id, or will not admit it to this bot.
    UnknownChannel,
    /// Discord rate-limited the probe, so readability was not established either way.
    RateLimited,
    /// Discord answered with a status this probe does not classify.
    UnexpectedStatus {
        /// HTTP status code.
        status: u16,
        /// Response body, already truncated by the client.
        body: String,
    },
    /// The request never reached Discord.
    Unreachable(String),
    /// Discord answered, but not in a shape this server understands.
    Unintelligible(String),
    /// The request was refused locally, before Discord was contacted.
    Refused(String),

    // ---------------------------------------------------------------------------------------
    // Vendor-neutral variants. Everything above this line began life describing one channel
    // read; everything below is spoken by the on-demand checks in `crate::diagnostics` as well,
    // and none of it names Discord.
    // ---------------------------------------------------------------------------------------
    /// A check that passed, carrying what it established.
    ///
    /// The counterpart of [`Diagnosis::Readable`] for a check that is not a channel read: "the
    /// key was accepted, and this is the account it reached", "this voice resolves, and here is
    /// where it came from". The note is the *evidence*, not decoration — a check that says only
    /// "ok" cannot be distinguished from one that did not run.
    Confirmed(String),
    /// A setting this check needs is absent, so nothing was called.
    ///
    /// Carries the setting's own name, exactly as
    /// [`crate::elevenlabs::SignedUrlError::NotConfigured`] and
    /// [`crate::store::StoreError::Unavailable`] do, because "which setting" is the whole answer.
    NotConfigured(&'static str),
    /// ElevenLabs rejected the account API key.
    KeyRejected,
    /// ElevenLabs does not have the configured agent on this account.
    UnknownAgent,
    /// No voice resolves: none was configured, and the agent has none of its own to borrow.
    NoVoice,
    /// The durable store cannot be used, for the reason it gave.
    StorageUnusable(String),
    /// The check did not finish inside its time budget.
    ///
    /// A FAILED CHECK with a reason, which is the entire point of having a budget: a diagnostics
    /// route that makes live vendor calls and has no deadline is a diagnostics route that hangs
    /// exactly when the vendor is the thing that is wrong.
    TimedOut(u64),
}

impl Diagnosis {
    /// Whether this diagnosis should stop the server from starting.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        !matches!(self, Self::Readable { .. } | Self::Confirmed(_))
    }

    /// Whether this diagnosis is worth saying out loud even though it is not a failure.
    #[must_use]
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            Self::Readable {
                suspect_message_content_intent: true,
                ..
            }
        )
    }

    /// A short statement of what is wrong.
    #[must_use]
    pub fn headline(&self) -> &'static str {
        match self {
            Self::Readable {
                suspect_message_content_intent: false,
                ..
            } => "readable",
            Self::Readable {
                suspect_message_content_intent: true,
                ..
            } => "readable, but every message came back with EMPTY CONTENT",
            Self::InvalidToken => "the provider rejected the bot token (HTTP 401)",
            Self::NoAccess => "the bot cannot see this channel at all (missing access)",
            Self::MissingReadHistory => {
                "the bot can see this channel but may not read its history \
                 (missing permissions)"
            }
            Self::UnknownChannel => "the provider does not know this channel id (HTTP 404)",
            Self::RateLimited => {
                "the provider rate-limited the probe (HTTP 429); readability is unknown"
            }
            Self::UnexpectedStatus { .. } => "the provider answered with an unexpected status",
            Self::Unreachable(_) => "the request never reached the vendor",
            Self::Unintelligible(_) => "the vendor's answer could not be understood",
            Self::Refused(_) => "the request was refused before it was sent",
            Self::Confirmed(_) => "confirmed",
            Self::NotConfigured(_) => "a setting this check needs is not configured",
            Self::KeyRejected => "ElevenLabs rejected the account API key (HTTP 401)",
            Self::UnknownAgent => {
                "ElevenLabs does not have this agent on the account the key belongs to (HTTP 404)"
            }
            Self::NoVoice => {
                "no voice resolves: none is configured, and the agent reports none of its own"
            }
            Self::StorageUnusable(_) => "the durable store cannot be written to",
            Self::TimedOut(_) => "the check did not answer inside its time budget",
        }
    }

    /// The headline with the checked provider's display name, supplied by its backend.
    #[must_use]
    pub fn headline_for(&self, provider: &str) -> String {
        let headline = self.headline();
        headline
            .strip_prefix("the provider")
            .map_or_else(|| headline.to_owned(), |rest| format!("{provider}{rest}"))
    }

    /// What the operator should actually do about it.
    ///
    /// A [`Cow`] rather than a `&'static str` because two of the remedies have to NAME something
    /// — the setting that is missing, the budget that elapsed — and a remedy that says "set the
    /// setting" without saying which one is the kind of unhelpful this whole module exists to
    /// delete. Everything with a fixed answer is still borrowed and allocates nothing.
    ///
    /// An empty string means "nothing to do", which is only ever the case for a check that
    /// passed.
    #[must_use]
    pub fn remedy(&self) -> Cow<'static, str> {
        self.remedy_for("the chat provider")
    }

    /// Advice for the configured provider, without giving another provider's setup steps.
    #[must_use]
    pub fn remedy_for(&self, provider: &str) -> Cow<'static, str> {
        // The detailed portal and permission instructions below apply only to Discord.
        // Other adapters may share its wire protocol, but they do not share its setup flow.
        if provider != "Discord" {
            let advice = match self {
                Self::Readable {
                    suspect_message_content_intent: true,
                    ..
                } => Some(format!(
                    "Check that {provider} grants this account access to message content. \
                     If access is correct, the recent messages may contain only attachments."
                )),
                Self::InvalidToken => Some(format!(
                    "Check the configured {provider} credential and renew or replace it if it \
                     expired or was revoked. No channel can be read until authentication works."
                )),
                Self::NoAccess => Some(format!(
                    "Check that the configured account has been added to the intended {provider} \
                     channel and is allowed to view it. Private channels may need an explicit invite."
                )),
                Self::MissingReadHistory => Some(format!(
                    "Grant the configured account permission to read message history in this \
                     {provider} channel. It can see the channel but cannot read its messages."
                )),
                Self::UnknownChannel => Some(format!(
                    "Check the channel identifier in VIBE_TALK_CHANNELS (or [[channels]]) against \
                     {provider}, and confirm that the configured account can access that channel."
                )),
                _ => None,
            };
            if let Some(advice) = advice {
                return Cow::Owned(advice);
            }
        }
        Cow::Borrowed(match self {
            Self::NotConfigured(setting) => {
                return Cow::Owned(format!(
                    "Set `{setting}` — in vibe-talk.toml, or through its VIBE_TALK_ environment \
                     variable — and restart the server. Nothing was called: this is an \
                     unconfigured capability rather than a broken one, and until the setting is \
                     there the feature is simply off."
                ))
            }
            Self::TimedOut(seconds) => {
                return Cow::Owned(format!(
                    "The vendor did not answer within {seconds}s, so this check proved nothing \
                     either way. Check egress from this host — DNS, outbound HTTPS, any proxy — \
                     and the vendor's own status page, then ask again. A repeatable timeout here \
                     is a network path that is down, not a slow one."
                ))
            }
            Self::Readable {
                suspect_message_content_intent: false,
                ..
            } => "",
            Self::Readable { .. } => {
                "Turn on MESSAGE CONTENT INTENT: Discord Developer Portal -> your application -> \
                 Bot -> Privileged Gateway Intents -> Message Content Intent -> Save Changes, then \
                 restart this server. Without it Discord returns every message with a blank \
                 content field and no error, so digests come back empty and nothing looks broken. \
                 If that toggle is already on, the channel's recent messages are genuinely \
                 attachment- or embed-only and this warning is expected."
            }
            Self::InvalidToken => {
                "The bot token is wrong, expired, or was regenerated. Discord Developer Portal -> \
                 your application -> Bot -> Reset Token, then set VIBE_TALK_DISCORD_BOT_TOKEN (or \
                 discord.bot_token) to the new value. This is global: no channel can be read until \
                 it is fixed."
            }
            Self::NoAccess => {
                "Two causes, in the order worth checking. (1) The bot is not in this Discord \
                 server: re-open the OAuth2 -> URL Generator invite (scope `bot`, permissions \
                 View Channels + Read Message History) and authorize it into the right server. \
                 (2) The bot IS in the server but cannot view this particular channel, which is \
                 what happens with a private channel: open the channel's Edit Channel -> \
                 Permissions and add the bot (or a role it has) explicitly. Server-wide \
                 permissions do not reach into a private channel."
            }
            Self::MissingReadHistory => {
                "Grant Read Message History to the bot on this channel: Edit Channel -> \
                 Permissions -> the bot or its role -> Read Message History. The bot is already in \
                 the server and can see the channel, so this is a per-channel permission override, \
                 not an invite problem."
            }
            Self::UnknownChannel => {
                "Check the channel id in VIBE_TALK_CHANNELS (or the [[channels]] table). Enable \
                 Discord's User Settings -> Advanced -> Developer Mode, right-click the channel, \
                 Copy Channel ID, and compare; it is a 17-20 digit number. Note that a channel in \
                 a server the bot was never invited to can also answer 404, so if the id is right, \
                 re-check the invite."
            }
            Self::RateLimited => {
                "Wait and try again. The client already waited the time the provider asked for, and \
                 at startup the probe goes on waiting for up to VIBE_TALK_STARTUP_PROBE_WAIT_SECONDS \
                 (five minutes unless set), so a limit that did not clear means something else is \
                 very likely sharing this credential and spending its rate limit."
            }
            Self::UnexpectedStatus { .. } => {
                "Read the status and body above. A 5xx means the provider or a gateway failed; \
                 check the provider and any adapter service, then retry the probe."
            }
            Self::Unreachable(_) => {
                "This process could not reach the provider. Check egress from the container or \
                 host: DNS, outbound HTTPS, and any proxy or adapter service."
            }
            Self::Unintelligible(_) => {
                "The provider returned something this server cannot parse. If its API endpoint \
                 points at a proxy or adapter, check that service first."
            }
            Self::Refused(_) => {
                "This is a bug in this server rather than a configuration problem: the probe only \
                 reads, and a read should not be refused locally. Please report it."
            }
            Self::Confirmed(_) => "",
            Self::KeyRejected => {
                "ElevenLabs did not accept the account API key. Create a fresh one at \
                 https://elevenlabs.io/app/settings/api-keys (Create API Key; it is shown once) \
                 and set elevenlabs.api_key, or VIBE_TALK_ELEVENLABS_API_KEY. A key that was \
                 revoked, or one copied with surrounding whitespace, fails exactly like this."
            }
            Self::UnknownAgent => {
                "The account this key belongs to has no agent with the configured id. Open \
                 https://elevenlabs.io/app/agents, click the agent you mean, and copy the id out \
                 of the dashboard URL into elevenlabs.agent_id. If the id looks right, the key \
                 belongs to a DIFFERENT account or workspace from the one holding the agent — \
                 check which workspace the elevenlabs.api_key check above reported."
            }
            Self::NoVoice => {
                "Either give the agent a voice — https://elevenlabs.io/app/agents, your agent, \
                 Voice — or name one directly in elevenlabs.voice_id. elevenlabs.voice_id is \
                 OPTIONAL precisely because read-aloud borrows the agent's own voice when it is \
                 unset; with neither present there is nothing to speak with."
            }
            Self::StorageUnusable(_) => {
                "The reason is in the detail above. storage.path must be an ABSOLUTE path to a \
                 file this process may write, in a directory it may also write (SQLite puts its \
                 journal beside the database). In a container that means a mounted volume: a \
                 relative path resolves inside the image and disappears on the next rebuild."
            }
        })
    }

    /// Extra context that is not fixed text — the status body, the transport error, and so on.
    #[must_use]
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::UnexpectedStatus { status, body } => Some(format!("HTTP {status}: {body}")),
            Self::Unreachable(detail)
            | Self::Unintelligible(detail)
            | Self::Refused(detail)
            | Self::Confirmed(detail)
            | Self::StorageUnusable(detail) => Some(detail.clone()),
            Self::NotConfigured(setting) => Some((*setting).to_owned()),
            _ => None,
        }
    }

    /// Whether this diagnosis was reached without calling anything.
    ///
    /// Worth its own question because "the vendor said no" and "we never asked" are the two
    /// answers a reader most needs to tell apart, and they otherwise look identical in a list of
    /// red rows.
    #[must_use]
    pub fn is_unconfigured(&self) -> bool {
        matches!(self, Self::NotConfigured(_))
    }
}

/// The probe's conclusion about one configured channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelProbe {
    /// The channel as configured, so the message can name the label the operator wrote.
    pub channel: ChannelInfo,
    /// What was concluded.
    pub diagnosis: Diagnosis,
}

/// The probe's conclusion about every configured channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// One entry per configured channel, in configuration order.
    ///
    /// Shorter than the channel list when the probe stopped early — see [`ProbeReport::aborted`].
    pub outcomes: Vec<ChannelProbe>,
    /// The probe stopped before checking every channel.
    ///
    /// Set when the bot token itself was rejected: once Discord has said the credential is bad,
    /// the remaining channels can only repeat the same 401, and hammering the endpoint to collect
    /// identical failures would be noise.
    pub aborted: bool,
}

impl ProbeReport {
    /// Whether the server should refuse to start.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        self.aborted || self.outcomes.iter().any(|o| o.diagnosis.is_failure())
    }

    /// The channels that failed.
    #[must_use]
    pub fn failures(&self) -> Vec<&ChannelProbe> {
        self.outcomes
            .iter()
            .filter(|o| o.diagnosis.is_failure())
            .collect()
    }

    /// The channels that were readable but suspicious.
    #[must_use]
    pub fn warnings(&self) -> Vec<&ChannelProbe> {
        self.outcomes
            .iter()
            .filter(|o| o.diagnosis.is_warning())
            .collect()
    }

    /// The whole report as operator-facing text.
    ///
    /// Every failing channel is named with its id, its configured label, what went wrong, and what
    /// to do about it. Passing channels get one line each, because "which of my channels did it
    /// actually check" is the first question a refusal raises.
    #[must_use]
    pub fn render(&self) -> String {
        self.render_for("the chat provider")
    }

    /// Render with the display name supplied by the checked chat backend.
    #[must_use]
    pub fn render_for(&self, provider: &str) -> String {
        let mut out = String::new();
        out.push_str("startup channel probe (a one-message read per configured channel):\n");
        for outcome in &self.outcomes {
            let mark = if outcome.diagnosis.is_failure() {
                "FAILED"
            } else if outcome.diagnosis.is_warning() {
                "WARNING"
            } else {
                "ok"
            };
            out.push_str(&format!(
                "  [{mark}] channel {} ({}): {}\n",
                outcome.channel.id,
                outcome.channel.label,
                outcome.diagnosis.headline_for(provider)
            ));
            if let Some(detail) = outcome.diagnosis.detail() {
                out.push_str(&format!("           detail: {detail}\n"));
            }
            let remedy = outcome.diagnosis.remedy_for(provider);
            if !remedy.is_empty() {
                out.push_str(&format!("           fix: {remedy}\n"));
            }
        }
        if self.aborted {
            out.push_str(
                "  probe stopped early: the bot token was rejected, so the remaining channels \
                 were not checked.\n",
            );
        }
        out
    }
}

/// Read Discord's JSON error code out of an error body, when there is one.
///
/// Discord's numeric `code` is more precise than the HTTP status — a 403 alone cannot tell "the
/// bot is not in this server" from "the bot lacks one permission here" — so it is preferred where
/// present and the status is the fallback.
fn discord_error_code(body: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("code")?
        .as_u64()
}

/// Classify one failed Discord call.
///
/// Public because [`crate::diagnostics`] classifies the bot-identity call — `GET /users/@me`,
/// which is how "is this token any good at all" is answered without a channel — and it must be
/// classified by THIS function rather than by a second copy of the same match: a token check that
/// disagreed with the channel checks about what a 401 means would be worse than no token check.
#[must_use]
pub fn classify(error: &ChatError) -> Diagnosis {
    match error.cause() {
        ChatError::Provider { .. } => unreachable!("cause removes provider wrappers"),
        ChatError::Transport(detail) => Diagnosis::Unreachable(detail.clone()),
        ChatError::Shape(detail) => Diagnosis::Unintelligible(detail.clone()),
        ChatError::Refused(detail) => Diagnosis::Refused(detail.clone()),
        // A probe reads no forward cursor, so this cannot arise from one; were it to, it is the
        // backend declining that read, which is what a refusal reports.
        ChatError::CursorExpired(detail) => Diagnosis::Refused(detail.clone()),
        // The client already waited this out and could not clear it, so the probe learned nothing
        // about readability — exactly what a bare 429 used to mean here, and still does.
        ChatError::RateLimited(_) => Diagnosis::RateLimited,
        ChatError::Status { status, body } => match (*status, discord_error_code(body)) {
            (401, _) => Diagnosis::InvalidToken,
            (_, Some(DISCORD_MISSING_ACCESS)) => Diagnosis::NoAccess,
            (_, Some(DISCORD_MISSING_PERMISSIONS)) => Diagnosis::MissingReadHistory,
            (_, Some(DISCORD_UNKNOWN_CHANNEL)) => Diagnosis::UnknownChannel,
            // No usable `code` field: fall back to the status, which is coarser but still says
            // something true.
            (403, _) => Diagnosis::NoAccess,
            (404, _) => Diagnosis::UnknownChannel,
            (429, _) => Diagnosis::RateLimited,
            (status, _) => Diagnosis::UnexpectedStatus {
                status,
                body: body.clone(),
            },
        },
    }
}

/// What one provider's own head read answered: remote-ops-audit W12 `#287 bulk-reads`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadsCheck {
    /// The provider's configuration key.
    pub key: String,
    /// The provider's display name.
    pub provider: String,
    /// [`Diagnosis::Confirmed`], saying how many channels were answered and how, when any was;
    /// otherwise the first failure, classified.
    pub diagnosis: Diagnosis,
}

/// Check the head read of every provider that reads heads natively, with one call each for the
/// channels of `channels` it holds; a provider whose heads are emulated is not asked, since its
/// head read is the probe's own read again. remote-ops-audit W12 `#287 bulk-reads`. Each call is
/// given `budget`, when there is one.
pub async fn check_heads(
    providers: &crate::providers::ChatRouter,
    channels: &[ChannelInfo],
    budget: Option<Duration>,
) -> Vec<HeadsCheck> {
    let mut checks = Vec::new();
    for entry in providers.entries() {
        let held: Vec<crate::model::ChannelId> = channels
            .iter()
            .filter(|channel| {
                providers.key_for(&channel.id).as_deref() == Some(entry.key.as_str())
                    && entry.client.read_caps(&channel.id).heads == crate::chat::Support::Native
            })
            .map(|channel| channel.id.clone())
            .collect();
        if held.is_empty() {
            continue;
        }
        let provider = entry.client.provider_name().to_owned();
        // `remote-ops-audit`: counted as the probe's, as the reads beside it are
        // (`#287 bulk-reads`).
        let read = crate::read_reason::ReadReason::Probe.scope(entry.client.channel_heads(&held));
        let diagnosis = match budget {
            Some(budget) => match tokio::time::timeout(budget, read).await {
                Ok(answers) => heads_diagnosis(&provider, &answers),
                Err(_elapsed) => Diagnosis::TimedOut(budget.as_secs()),
            },
            None => heads_diagnosis(&provider, &read.await),
        };
        checks.push(HeadsCheck {
            key: entry.key.clone(),
            provider,
            diagnosis,
        });
    }
    checks
}

/// What a head read's answers say about it, in the shared vocabulary.
fn heads_diagnosis(provider: &str, answers: &crate::chat::HeadAnswers) -> Diagnosis {
    let heads: Vec<&crate::chat::ChannelHead> = answers
        .iter()
        .filter_map(|(_, head)| head.as_ref().ok())
        .collect();
    let first_failure = answers.iter().find_map(|(_, head)| head.as_ref().err());
    if heads.is_empty() {
        if let Some(error) = first_failure {
            return classify(error);
        }
    }
    let mut note = format!(
        "{provider} answered the newest message of {} of {} channel(s) in one head read, so the \
         live poller skips the pages of its quiet channels",
        heads.len(),
        answers.len()
    );
    let vouching = heads
        .iter()
        .filter(|head| head.is_current() && head.unchanged_since_ms.is_some())
        .count();
    if vouching > 0 {
        note.push_str(&format!(
            "; {vouching} also vouch that nothing changed since an instant, so a catch-up can \
             keep its held copy of them"
        ));
    }
    let stale = heads.iter().filter(|head| !head.is_current()).count();
    if stale > 0 {
        note.push_str(&format!(
            "; {stale} came from an older copy, which nothing acts on"
        ));
    }
    if let Some(error) = first_failure {
        note.push_str(&format!(
            "; the others are read as before heads, the first because: {error}"
        ));
    }
    Diagnosis::Confirmed(note)
}

/// Probe every configured channel.
///
/// Reads one message per channel and never writes. Returns a report rather than an error: the
/// caller decides what a failure means, and the report is the thing worth logging either way.
pub async fn probe_channels(chat: &dyn ChatClient, channels: &[ChannelInfo]) -> ProbeReport {
    probe_channels_within(chat, channels, None).await
}

/// Probe every configured channel, giving each read at most `budget`.
///
/// The same loop as [`probe_channels`], with a deadline. Startup passes `None` — a server that is
/// refusing to come up can afford to wait for a real answer, and the HTTP client already carries
/// its own timeout. The on-demand route in [`crate::diagnostics`] passes a budget, because it is
/// answering a request somebody is waiting on and "the vendor never replied" has to arrive as a
/// failed check rather than as a request that never ends.
///
/// A budget that elapses yields [`Diagnosis::TimedOut`] for that channel and the loop CONTINUES:
/// one unreachable channel must not hide the state of the others.
pub async fn probe_channels_within(
    chat: &dyn ChatClient,
    channels: &[ChannelInfo],
    budget: Option<Duration>,
) -> ProbeReport {
    probe_round(chat, channels, budget).await.0
}

/// One read of each channel, with what the provider asked for beside each outcome that was a rate
/// limit naming a time. Kept beside the report rather than in it, because the report speaks the
/// vocabulary every check shares and only the startup probe waits.
async fn probe_round(
    chat: &dyn ChatClient,
    channels: &[ChannelInfo],
    budget: Option<Duration>,
) -> (ProbeReport, Vec<Option<Duration>>) {
    let mut outcomes = Vec::with_capacity(channels.len());
    let mut asked = Vec::with_capacity(channels.len());
    let mut aborted = false;
    for channel in channels {
        let (diagnosis, retry_after) = probe_one(chat, channel, budget).await;
        let is_token_failure = diagnosis == Diagnosis::InvalidToken;
        outcomes.push(ChannelProbe {
            channel: channel.clone(),
            diagnosis,
        });
        asked.push(retry_after);
        if is_token_failure {
            aborted = true;
            break;
        }
    }
    (ProbeReport { outcomes, aborted }, asked)
}

/// Read one channel's newest message and classify the answer, with how long the provider asked
/// to be left alone when the answer was a rate limit and it named a time.
async fn probe_one(
    chat: &dyn ChatClient,
    channel: &ChannelInfo,
    budget: Option<Duration>,
) -> (Diagnosis, Option<Duration>) {
    let read =
        crate::read_reason::ReadReason::Probe.scope(chat.probe_channel(&channel.id, PROBE_LIMIT));
    let answered = match budget {
        Some(budget) => match tokio::time::timeout(budget, read).await {
            Ok(answered) => answered,
            Err(_elapsed) => return (Diagnosis::TimedOut(budget.as_secs()), None),
        },
        None => read.await,
    };
    match answered {
        Ok(messages) => (
            Diagnosis::Readable {
                messages: messages.len(),
                // Vacuously true on an empty channel would be a false alarm, so an empty result
                // is not evidence of anything: require at least one message before suspecting the
                // intent toggle.
                suspect_message_content_intent: !messages.is_empty()
                    && messages.iter().all(|m| m.content.trim().is_empty()),
            },
            None,
        ),
        Err(error) => (classify(&error), error.retry_after()),
    }
}

/// Whether the startup check ran, and what it concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupCheck {
    /// The probe was deliberately not run.
    Skipped {
        /// Which switch caused it — the flag name or the environment variable name.
        ///
        /// Carried so the caller can say *which* one, rather than logging a bare "skipped" that
        /// leaves the operator hunting for a switch they may not have set themselves.
        source: &'static str,
    },
    /// The probe ran and every channel is readable. May still carry warnings.
    Passed(ProbeReport),
    /// The probe ran and at least one channel is definitely not readable: a rejected token, a
    /// missing channel, a missing permission, or anything else that is not a rate limit.
    Failed(ProbeReport),
    /// The provider was still rate-limiting the probe when its wait budget ran out, so
    /// readability was never established. `#718 vt-startup-probe-restart`.
    RateLimited {
        /// The last round's report: every channel still rate-limited says so.
        report: ProbeReport,
        /// How long the probe had been running when it gave up.
        waited: Duration,
    },
}

/// Whether an environment value asks for the probe to be skipped.
///
/// `0`, `false`, and the empty string do not, so a container runtime that renders an unset
/// variable as `""` cannot silently disable the check.
#[must_use]
pub fn env_requests_skip(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

/// Environment variable bounding how long the startup probe waits out a provider's rate limit,
/// in whole seconds. `#718 vt-startup-probe-restart`.
pub const ENV_STARTUP_PROBE_WAIT: &str = "VIBE_TALK_STARTUP_PROBE_WAIT_SECONDS";

/// Command-line flag for the same bound; it wins over [`ENV_STARTUP_PROBE_WAIT`].
pub const WAIT_FLAG: &str = "--startup-probe-wait";

/// How long the startup probe waits out rate limits when nothing says otherwise.
pub const DEFAULT_RATE_LIMIT_WAIT: Duration = Duration::from_secs(300);

/// The longest wait budget accepted: one day. Longer is a typo, and a deadline that far out
/// would not fit in a clock reading.
pub const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(86_400);

/// The startup probe's wait budget from the flag's value, else the environment's, else
/// [`DEFAULT_RATE_LIMIT_WAIT`]. An empty environment value counts as unset, as it does for the
/// skip; `0` means "do not wait": the first rate limit stops the server, as it did before the
/// probe learned to wait.
///
/// # Errors
///
/// The setting's name and the value, when the value is not a whole number of seconds up to
/// [`MAX_RATE_LIMIT_WAIT`].
pub fn wait_budget(flag: Option<&str>, env: Option<&str>) -> Result<Duration, String> {
    let (name, value) = match (flag, env) {
        (Some(value), _) => (WAIT_FLAG, value),
        (None, Some(value)) if !value.trim().is_empty() => (ENV_STARTUP_PROBE_WAIT, value),
        _ => return Ok(DEFAULT_RATE_LIMIT_WAIT),
    };
    value
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .filter(|budget| *budget <= MAX_RATE_LIMIT_WAIT)
        .ok_or_else(|| {
            format!(
                "{name} must be a whole number of seconds, at most {}, not {value:?}",
                MAX_RATE_LIMIT_WAIT.as_secs()
            )
        })
}

/// How the startup probe waits out a provider's rate limit. `#718 vt-startup-probe-restart`.
///
/// A deploy restarts the server just after the previous process spent the provider's read
/// budget, so the first probe is often refused with a rate limit that clears in seconds. Treated
/// as a failure, it stopped the server, whose supervisor restarted it, whose probe spent the
/// budget again. So a rate limit is waited out instead: for the time the provider names, or else
/// for a growing, jittered backoff; and only the channels that were limited are read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPatience {
    /// The most time the probe may run, counted from its first read, before it gives up.
    pub budget: Duration,
    /// The first wait when the provider names no time; each later one doubles it.
    pub first_backoff: Duration,
    /// The longest wait when the provider names no time.
    pub max_backoff: Duration,
}

impl RateLimitPatience {
    /// Wait at most `budget` in all, backing off from 2 s up to 60 s when the provider names no
    /// time.
    #[must_use]
    pub fn within(budget: Duration) -> Self {
        Self {
            budget,
            first_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(60),
        }
    }

    /// The wait before re-reading after the `round`th rate-limited round (from 0): what the
    /// provider asked for when it named a time, else the backoff for that round scaled by
    /// `jitter` (in `[0, 1]`) into its upper half, so that it is never zero and never past
    /// `max_backoff`.
    #[must_use]
    pub fn next_wait(&self, asked: Option<Duration>, round: u32, jitter: f64) -> Duration {
        if let Some(asked) = asked.filter(|asked| !asked.is_zero()) {
            return asked;
        }
        let doubled = self
            .first_backoff
            .saturating_mul(1_u32 << round.min(16))
            .min(self.max_backoff);
        doubled.mul_f64(0.5 + 0.5 * jitter.clamp(0.0, 1.0))
    }
}

impl Default for RateLimitPatience {
    fn default() -> Self {
        Self::within(DEFAULT_RATE_LIMIT_WAIT)
    }
}

/// A number in `[0, 1]` from the system's random source, so that servers restarted together do
/// not retry together; `1.0`, the plain backoff, when there is no random source.
fn jitter() -> f64 {
    let mut bytes = [0_u8; 4];
    match getrandom::fill(&mut bytes) {
        Ok(()) => f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX),
        Err(_) => 1.0,
    }
}

/// Where the startup probe stands, for `/healthz` and `/readyz` and for the background work
/// that should not read the provider before it is done. `#718 vt-startup-probe-restart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPhase {
    /// No startup probe belongs to this server: one built by a test, say.
    NotRun,
    /// Reading each configured channel.
    Probing,
    /// The provider rate-limited the probe; it reads the limited channels again at `retry_at`.
    RateLimited {
        /// When the next read is due.
        retry_at: tokio::time::Instant,
    },
    /// Every configured channel was readable.
    Passed,
    /// The probe was switched off.
    Skipped,
    /// The probe failed, or gave up waiting, and the server is stopping.
    Failed,
}

impl StartupPhase {
    /// Whether the probe is done, one way or the other.
    #[must_use]
    pub fn is_settled(self) -> bool {
        !matches!(self, Self::Probing | Self::RateLimited { .. })
    }

    /// Whether the server is ready: its probe passed, was skipped, or was never part of it.
    #[must_use]
    pub fn is_ready(self) -> bool {
        matches!(self, Self::NotRun | Self::Passed | Self::Skipped)
    }

    /// The overall word for `/healthz`: `ok` when ready, `starting` while the probe is pending,
    /// `failed` once it has failed.
    #[must_use]
    pub fn status(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            phase if phase.is_ready() => "ok",
            _ => "starting",
        }
    }

    /// What the probe is doing, as `/healthz` says it: `probe pending (rate limited, retry in
    /// 12s)` while it waits out a rate limit.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::NotRun => "not run".to_owned(),
            Self::Probing => "probe pending (reading each configured channel)".to_owned(),
            Self::RateLimited { retry_at } => {
                let left = retry_at.saturating_duration_since(tokio::time::Instant::now());
                format!(
                    "probe pending (rate limited, retry in {}s)",
                    left.as_secs() + u64::from(left.subsec_nanos() > 0)
                )
            }
            Self::Passed => "passed".to_owned(),
            Self::Skipped => "skipped".to_owned(),
            Self::Failed => "failed; the server is stopping".to_owned(),
        }
    }
}

/// The startup probe's [`StartupPhase`], shared between the probe and whoever asks.
#[derive(Debug)]
pub struct StartupStatus(tokio::sync::watch::Sender<StartupPhase>);

impl StartupStatus {
    /// A status in `phase`.
    #[must_use]
    pub fn new(phase: StartupPhase) -> Self {
        Self(tokio::sync::watch::Sender::new(phase))
    }

    /// The phase now.
    #[must_use]
    pub fn phase(&self) -> StartupPhase {
        *self.0.borrow()
    }

    /// Move to `phase`.
    pub fn set(&self, phase: StartupPhase) {
        self.0.send_replace(phase);
    }

    /// Return once the probe is done, one way or the other; at once when it already is.
    pub async fn settled(&self) {
        let mut phases = self.0.subscribe();
        // The sender lives in `self`, so it cannot be dropped while this waits.
        let _ = phases.wait_for(|phase| phase.is_settled()).await;
    }

    /// Wait until the probe is done, and say whether the server is ready: `false` once it
    /// failed, when the server is stopping and background work should not start reading.
    pub async fn ready(&self) -> bool {
        self.settled().await;
        self.phase().is_ready()
    }
}

impl Default for StartupStatus {
    fn default() -> Self {
        Self::new(StartupPhase::NotRun)
    }
}

/// Run the startup check once, honouring the skip switches; a rate limit fails it at once.
///
/// The skip is resolved here rather than at the call site so that "was it skipped, and by what"
/// is a value a test can assert on, instead of a branch that only exists inside `main`.
pub async fn startup_check(
    chat: &dyn ChatClient,
    channels: &[ChannelInfo],
    skip_flag: bool,
    skip_env: Option<&str>,
) -> StartupCheck {
    startup_check_patiently(
        chat,
        channels,
        skip_flag,
        skip_env,
        RateLimitPatience::within(Duration::ZERO),
        &StartupStatus::default(),
    )
    .await
}

/// Run the startup check, honouring the skip switches, waiting out provider rate limits within
/// `patience`, and keeping `status` current throughout. `#718 vt-startup-probe-restart`.
///
/// A definite failure (a rejected token, a missing channel or permission, anything that is not a
/// rate limit) ends it at once, as it always did. A round in which the only failures are rate
/// limits is followed by a wait, logged at WARN with the budget left, and a second read of just
/// those channels. When the next wait would not fit in what is left of the budget, it gives up
/// with [`StartupCheck::RateLimited`]: a wait the provider named that is longer than the time
/// left is refused before it is taken, and a backoff of this server's choosing is cut short to
/// make one last read at the end of the budget.
pub async fn startup_check_patiently(
    chat: &dyn ChatClient,
    channels: &[ChannelInfo],
    skip_flag: bool,
    skip_env: Option<&str>,
    patience: RateLimitPatience,
    status: &StartupStatus,
) -> StartupCheck {
    if skip_flag {
        status.set(StartupPhase::Skipped);
        return StartupCheck::Skipped { source: SKIP_FLAG };
    }
    if env_requests_skip(skip_env) {
        status.set(StartupPhase::Skipped);
        return StartupCheck::Skipped {
            source: ENV_SKIP_STARTUP_PROBE,
        };
    }
    status.set(StartupPhase::Probing);
    let started = tokio::time::Instant::now();
    let deadline = started + patience.budget;
    let provider = chat.provider_name();
    // With time to wait, the probe does the waiting: a client that can give up at a provider's
    // first rate limit does, so `/healthz` says the probe is rate-limited while it waits and the
    // wait counts against this budget. One that cannot (Discord's limiter waits inside the read,
    // up to its own 30 s) waits there first, so the budget can be overrun by one round of those.
    let hurried = !patience.budget.is_zero();
    let (mut report, mut asked) =
        crate::read_reason::hurry(hurried, probe_round(chat, channels, None)).await;
    let mut round = 0_u32;
    loop {
        let limited: Vec<usize> = report
            .outcomes
            .iter()
            .enumerate()
            .filter(|(_, outcome)| outcome.diagnosis == Diagnosis::RateLimited)
            .map(|(index, _)| index)
            .collect();
        let definite = report.aborted
            || report
                .outcomes
                .iter()
                .any(|o| o.diagnosis.is_failure() && o.diagnosis != Diagnosis::RateLimited);
        if definite {
            status.set(StartupPhase::Failed);
            return StartupCheck::Failed(report);
        }
        if limited.is_empty() {
            status.set(StartupPhase::Passed);
            return StartupCheck::Passed(report);
        }
        let named = limited
            .iter()
            .filter_map(|&index| asked[index])
            .filter(|asked| !asked.is_zero())
            .max();
        let now = tokio::time::Instant::now();
        let left = deadline.saturating_duration_since(now);
        let mut wait = patience.next_wait(named, round, jitter());
        if wait > left {
            if named.is_some() || left.is_zero() {
                status.set(StartupPhase::Failed);
                return StartupCheck::RateLimited {
                    report,
                    waited: now - started,
                };
            }
            wait = left;
        }
        tracing::warn!(
            provider,
            rate_limited = limited.len(),
            configured = channels.len(),
            wait_seconds = wait.as_secs_f64(),
            asked_by_provider = named.is_some(),
            budget_left_seconds = (left - wait).as_secs(),
            budget_seconds = patience.budget.as_secs(),
            "the startup channel probe was RATE LIMITED; waiting, then reading only the limited \
             channels again. The server is already serving meanwhile."
        );
        status.set(StartupPhase::RateLimited {
            retry_at: now + wait,
        });
        tokio::time::sleep(wait).await;
        status.set(StartupPhase::Probing);
        for &index in &limited {
            let channel = report.outcomes[index].channel.clone();
            let (diagnosis, retry_after) =
                crate::read_reason::hurry(hurried, probe_one(chat, &channel, None)).await;
            let is_token_failure = diagnosis == Diagnosis::InvalidToken;
            report.outcomes[index].diagnosis = diagnosis;
            asked[index] = retry_after;
            if is_token_failure {
                report.aborted = true;
                break;
            }
        }
        round += 1;
    }
}

/// Command-line flag that skips the startup probe.
pub const SKIP_FLAG: &str = "--skip-startup-probe";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::fake::FakeDiscord;
    use crate::model::{ChannelId, ChannelInfo};
    use std::sync::Arc;

    fn info(id: &str, label: &str, writable: bool) -> ChannelInfo {
        ChannelInfo {
            id: ChannelId(id.to_owned()),
            label: label.to_owned(),
            writable,
            alias: None,
            added: false,
            provider: None,
            write_source: None,
        }
    }

    fn status(status: u16, body: &str) -> ChatError {
        ChatError::Status {
            status,
            body: body.to_owned(),
        }
    }

    #[tokio::test]
    async fn a_registered_channel_probes_clean() {
        let fake = FakeDiscord::new();
        fake.register_channel(&ChannelId("111".to_owned()));
        let report = probe_channels(&fake, &[info("111", "lead team", true)]).await;
        assert!(!report.is_failure(), "{}", report.render());
        assert!(report.failures().is_empty());
    }

    #[tokio::test]
    async fn an_unregistered_channel_fails_the_probe() {
        // The whole point of the fake refusing an unconfigured channel: if it answered every
        // fetch with an empty success, this probe would pass against it unconditionally and prove
        // nothing about the real client.
        let fake = FakeDiscord::new();
        let report = probe_channels(&fake, &[info("404404404", "typo'd", false)]).await;
        assert!(report.is_failure(), "{}", report.render());
        assert_eq!(report.failures().len(), 1);
        assert_eq!(report.outcomes[0].diagnosis, Diagnosis::UnknownChannel);
        let rendered = report.render_for(fake.provider_name());
        assert!(rendered.contains("404404404"), "{rendered}");
        assert!(rendered.contains("typo'd"), "{rendered}");
        assert!(rendered.contains("Copy Channel ID"), "{rendered}");
    }

    #[tokio::test]
    async fn the_probe_never_posts_even_to_a_writable_channel() {
        let fake = FakeDiscord::new();
        let channel = ChannelId("222".to_owned());
        fake.register_channel(&channel);
        fake.seed(&channel, "codex-eng", "already here");
        let report = probe_channels(&fake, &[info("222", "lead team", true)]).await;
        assert!(!report.is_failure(), "{}", report.render());
        assert!(
            fake.posted().is_empty(),
            "the startup probe must be read-only: {:?}",
            fake.posted()
        );
    }

    #[tokio::test]
    async fn a_transport_failure_is_reported_as_unreachable() {
        let fake = FakeDiscord::new();
        let channel = ChannelId("333".to_owned());
        fake.register_channel(&channel);
        fake.fail_next("dns failure");
        let report = probe_channels(&fake, &[info("333", "ops", false)]).await;
        assert!(report.is_failure());
        assert!(matches!(
            report.outcomes[0].diagnosis,
            Diagnosis::Unreachable(_)
        ));
        assert!(report.render().contains("dns failure"));
    }

    #[tokio::test]
    async fn every_configured_channel_is_probed_and_each_failure_is_named() {
        let fake = FakeDiscord::new();
        fake.register_channel(&ChannelId("1".to_owned()));
        let report = probe_channels(
            &fake,
            &[
                info("1", "reachable", false),
                info("2", "missing one", false),
                info("3", "missing two", true),
            ],
        )
        .await;
        assert_eq!(report.outcomes.len(), 3);
        let failures = report.failures();
        assert_eq!(failures.len(), 2);
        let rendered = report.render();
        assert!(rendered.contains("missing one"), "{rendered}");
        assert!(rendered.contains("missing two"), "{rendered}");
    }

    #[test]
    fn each_discord_signal_maps_to_its_own_cause() {
        assert_eq!(
            classify(&status(401, r#"{"message":"401: Unauthorized","code":0}"#)),
            Diagnosis::InvalidToken
        );
        assert_eq!(
            classify(&status(403, r#"{"message":"Missing Access","code":50001}"#)),
            Diagnosis::NoAccess
        );
        assert_eq!(
            classify(&status(
                403,
                r#"{"message":"Missing Permissions","code":50013}"#
            )),
            Diagnosis::MissingReadHistory
        );
        assert_eq!(
            classify(&status(
                404,
                r#"{"message":"Unknown Channel","code":10003}"#
            )),
            Diagnosis::UnknownChannel
        );
        assert_eq!(classify(&status(429, "{}")), Diagnosis::RateLimited);
        assert!(matches!(
            classify(&status(503, "<html>gateway</html>")),
            Diagnosis::UnexpectedStatus { status: 503, .. }
        ));
    }

    #[test]
    fn provider_diagnostics_do_not_inherit_another_platforms_setup_instructions() {
        for diagnosis in [
            Diagnosis::Readable {
                messages: 1,
                suspect_message_content_intent: true,
            },
            Diagnosis::InvalidToken,
            Diagnosis::NoAccess,
            Diagnosis::MissingReadHistory,
            Diagnosis::UnknownChannel,
            Diagnosis::RateLimited,
            Diagnosis::UnexpectedStatus {
                status: 502,
                body: "gateway failure".into(),
            },
            Diagnosis::Unreachable("offline".into()),
            Diagnosis::Unintelligible("invalid response".into()),
        ] {
            for text in [
                diagnosis.headline().to_owned(),
                diagnosis.remedy().into_owned(),
                diagnosis.headline_for("Google Chat"),
                diagnosis.remedy_for("Google Chat").into_owned(),
            ] {
                assert!(!text.to_lowercase().contains("discord"), "{text}");
            }
        }
        let report = ProbeReport {
            outcomes: vec![ChannelProbe {
                channel: info("1", "team", true),
                diagnosis: classify(&status(401, "").with_provider("Google Chat")),
            }],
            aborted: true,
        };
        let rendered = report.render_for("Google Chat");
        assert!(
            rendered.contains("Google Chat rejected the bot token"),
            "{rendered}"
        );
        assert!(!rendered.contains("Discord"), "{rendered}");
        assert!(!rendered.contains("Reset Token"), "{rendered}");
    }

    #[test]
    fn missing_access_and_missing_permissions_give_different_advice() {
        // If these two ever collapse into one message the probe stops being worth having: one is
        // "invite the bot", the other is "the invite is fine, grant one permission".
        let no_access = Diagnosis::NoAccess;
        let no_history = Diagnosis::MissingReadHistory;
        assert_ne!(no_access.remedy(), no_history.remedy());
        assert!(no_access.remedy_for("Discord").contains("OAuth2"));
        assert!(no_history
            .remedy_for("Discord")
            .contains("Read Message History"));
        assert!(
            !no_history.remedy_for("Discord").contains("OAuth2"),
            "a permission-override problem must not send the operator back to the invite flow"
        );
    }

    #[test]
    fn a_bad_token_stops_the_probe_rather_than_repeating_itself() {
        // Not a per-channel fault, so it must not be reported once per channel.
        assert!(Diagnosis::InvalidToken.is_failure());
        assert!(Diagnosis::InvalidToken
            .remedy_for("Discord")
            .contains("Reset Token"));
    }

    #[tokio::test]
    async fn a_rejected_token_aborts_the_remaining_channels() {
        struct AlwaysUnauthorized;
        #[async_trait::async_trait]
        impl ChatClient for AlwaysUnauthorized {
            async fn identity(&self) -> Result<crate::chat::ChatIdentity, ChatError> {
                Err(status(401, r#"{"message":"401: Unauthorized","code":0}"#))
            }
            async fn fetch_page(
                &self,
                _channel: &ChannelId,
                _limit: u16,
                _before: Option<&crate::model::MessageId>,
                _after: Option<&crate::model::MessageId>,
            ) -> Result<Vec<crate::model::Message>, ChatError> {
                Err(status(401, r#"{"message":"401: Unauthorized","code":0}"#))
            }
            async fn post_message(
                &self,
                _channel: &ChannelId,
                _content: &str,
                _reply_to: Option<&crate::model::MessageId>,
            ) -> Result<crate::model::Message, ChatError> {
                panic!("the startup probe must never post");
            }
        }
        let report = probe_channels(
            &AlwaysUnauthorized,
            &[info("1", "a", false), info("2", "b", false)],
        )
        .await;
        assert!(report.aborted);
        assert_eq!(report.outcomes.len(), 1, "{}", report.render());
        assert!(report.is_failure());
        assert!(report.render().contains("stopped early"));
    }

    #[tokio::test]
    async fn blank_content_is_flagged_as_the_message_content_intent() {
        // The highest-cost misconfiguration available, because Discord reports it as success.
        struct BlankContent;
        #[async_trait::async_trait]
        impl ChatClient for BlankContent {
            async fn identity(&self) -> Result<crate::chat::ChatIdentity, ChatError> {
                Ok(crate::chat::ChatIdentity {
                    id: "3000000000000000002".to_owned(),
                    username: "blank-content-bot".to_owned(),
                    workspace: None,
                })
            }
            async fn fetch_page(
                &self,
                channel: &ChannelId,
                _limit: u16,
                _before: Option<&crate::model::MessageId>,
                _after: Option<&crate::model::MessageId>,
            ) -> Result<Vec<crate::model::Message>, ChatError> {
                Ok(vec![crate::model::Message {
                    edited_timestamp: None,
                    thread: None,
                    id: crate::model::MessageId("1".to_owned()),
                    channel_id: channel.clone(),
                    author: "codex-eng".to_owned(),
                    author_id: crate::model::UserId("2000000000000000001".to_owned()),
                    author_is_bot: true,
                    timestamp: "2026-08-18T12:00:00+00:00".to_owned(),
                    spoken_time: String::new(),
                    reply_to: None,
                    content: String::new(),
                    spoken_content: String::new(),
                    content_html: String::new(),
                    noise: false,
                    reactions: None,
                    heading: None,
                    mentions: Vec::new(),
                    upstream_url: None,
                    upstream_app_url: None,
                    op_key: None,
                    observed_ms: None,
                }])
            }
            async fn post_message(
                &self,
                _channel: &ChannelId,
                _content: &str,
                _reply_to: Option<&crate::model::MessageId>,
            ) -> Result<crate::model::Message, ChatError> {
                panic!("the startup probe must never post");
            }
        }
        let report = probe_channels(&BlankContent, &[info("1", "lead team", false)]).await;
        assert!(
            !report.is_failure(),
            "a blank backlog is suspicious, not provably wrong: {}",
            report.render()
        );
        assert_eq!(report.warnings().len(), 1);
        let rendered = report.render_for("Discord");
        assert!(rendered.contains("EMPTY CONTENT"), "{rendered}");
        assert!(rendered.contains("MESSAGE CONTENT INTENT"), "{rendered}");
    }

    #[tokio::test]
    async fn the_skip_flag_bypasses_the_probe_entirely() {
        // An unregistered channel: without the skip this would FAIL, so a pass here can only mean
        // the probe was really bypassed and not merely lenient.
        let fake = FakeDiscord::new();
        let channels = [info("1", "never invited", false)];
        assert!(matches!(
            startup_check(&fake, &channels, false, None).await,
            StartupCheck::Failed(_)
        ));

        let fake = FakeDiscord::new();
        let checked = startup_check(&fake, &channels, true, None).await;
        assert_eq!(
            checked,
            StartupCheck::Skipped { source: SKIP_FLAG },
            "the skip must be attributable to the switch that caused it"
        );
        assert_eq!(
            fake.fetch_count(),
            0,
            "a skipped probe must not read anything at all"
        );
    }

    #[tokio::test]
    async fn the_environment_variable_also_skips_it() {
        let fake = FakeDiscord::new();
        let channels = [info("1", "never invited", false)];
        let checked = startup_check(&fake, &channels, false, Some("1")).await;
        assert_eq!(
            checked,
            StartupCheck::Skipped {
                source: ENV_SKIP_STARTUP_PROBE
            }
        );
        assert_eq!(fake.fetch_count(), 0);
    }

    #[test]
    fn an_unset_or_false_environment_value_does_not_skip() {
        // A container runtime that renders an unset variable as "" must not turn the check off.
        assert!(!env_requests_skip(None));
        assert!(!env_requests_skip(Some("")));
        assert!(!env_requests_skip(Some("0")));
        assert!(!env_requests_skip(Some("false")));
        assert!(!env_requests_skip(Some("False")));
        assert!(env_requests_skip(Some("1")));
        assert!(env_requests_skip(Some("yes")));
    }

    #[tokio::test]
    async fn an_empty_channel_is_not_mistaken_for_a_disabled_intent() {
        let fake = FakeDiscord::new();
        fake.register_channel(&ChannelId("1".to_owned()));
        let report = probe_channels(&fake, &[info("1", "quiet", false)]).await;
        assert!(report.warnings().is_empty(), "{}", report.render());
        assert_eq!(
            report.outcomes[0].diagnosis,
            Diagnosis::Readable {
                messages: 0,
                suspect_message_content_intent: false
            }
        );
    }

    /// One scripted answer: given the channel and how many times it was read before.
    type Script =
        dyn Fn(&ChannelId, u32) -> Result<Vec<crate::model::Message>, ChatError> + Send + Sync;

    /// `#718 vt-startup-probe-restart`. A chat client whose every read is answered by a script,
    /// given the channel and how many times that channel has been read before.
    struct Scripted {
        answer: Box<Script>,
        reads: std::sync::Mutex<std::collections::HashMap<String, u32>>,
    }

    impl Scripted {
        fn new(
            answer: impl Fn(&ChannelId, u32) -> Result<Vec<crate::model::Message>, ChatError>
                + Send
                + Sync
                + 'static,
        ) -> Self {
            Self {
                answer: Box::new(answer),
                reads: std::sync::Mutex::default(),
            }
        }

        fn reads_of(&self, channel: &str) -> u32 {
            self.reads
                .lock()
                .expect("reads")
                .get(channel)
                .copied()
                .unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl ChatClient for Scripted {
        async fn identity(&self) -> Result<crate::chat::ChatIdentity, ChatError> {
            Ok(crate::chat::ChatIdentity {
                id: "U1".to_owned(),
                username: "probe-test".to_owned(),
                workspace: None,
            })
        }
        async fn fetch_page(
            &self,
            channel: &ChannelId,
            _limit: u16,
            _before: Option<&crate::model::MessageId>,
            _after: Option<&crate::model::MessageId>,
        ) -> Result<Vec<crate::model::Message>, ChatError> {
            let before = {
                let mut reads = self.reads.lock().expect("reads");
                let count = reads.entry(channel.0.clone()).or_insert(0);
                *count += 1;
                *count - 1
            };
            (self.answer)(channel, before)
        }
        async fn post_message(
            &self,
            _channel: &ChannelId,
            _content: &str,
            _reply_to: Option<&crate::model::MessageId>,
        ) -> Result<crate::model::Message, ChatError> {
            panic!("the startup probe must never post");
        }
    }

    /// What a rate-limited read returns once the client has waited out its own budget: the
    /// provider's `retry_after` when it named one, a bare 429 when it did not.
    fn limited(retry_after: Option<Duration>) -> ChatError {
        match retry_after {
            Some(retry_after) => ChatError::RateLimited(crate::chat::RateLimitExhausted {
                provider: "slack",
                route: "conversations.history".to_owned(),
                attempts: 1,
                waited: Duration::ZERO,
                budget: Duration::from_secs(30),
                retry_after,
                global: false,
            })
            .with_provider("Slack"),
            None => status(429, "{}"),
        }
    }

    fn patient(seconds: u64) -> RateLimitPatience {
        RateLimitPatience::within(Duration::from_secs(seconds))
    }

    async fn patiently(
        chat: &dyn ChatClient,
        channels: &[ChannelInfo],
        patience: RateLimitPatience,
        status: &StartupStatus,
    ) -> StartupCheck {
        startup_check_patiently(chat, channels, false, None, patience, status).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_probe_waits_as_the_provider_asks_and_then_passes() {
        let chat = Scripted::new(|_, before| {
            if before < 3 {
                Err(limited(Some(Duration::from_secs(7))))
            } else {
                Ok(Vec::new())
            }
        });
        let status = StartupStatus::new(StartupPhase::Probing);
        let started = tokio::time::Instant::now();
        let checked = patiently(&chat, &[info("C1", "team", true)], patient(300), &status).await;
        assert!(matches!(checked, StartupCheck::Passed(_)), "{checked:?}");
        assert_eq!(chat.reads_of("C1"), 4);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(21),
            "three waits of exactly the 7 s the provider asked for"
        );
        assert_eq!(status.phase(), StartupPhase::Passed);
    }

    #[tokio::test(start_paused = true)]
    async fn a_probe_rate_limited_forever_gives_up_inside_its_budget() {
        let chat = Scripted::new(|_, _| Err(limited(Some(Duration::from_secs(40)))));
        let status = StartupStatus::new(StartupPhase::Probing);
        let started = tokio::time::Instant::now();
        let checked = patiently(&chat, &[info("C1", "team", true)], patient(100), &status).await;
        let StartupCheck::RateLimited { report, waited } = checked else {
            panic!("expected the probe to give up on the rate limit, got {checked:?}");
        };
        // 40 s, 40 s, and a third 40 s would pass the 100 s budget, so it is refused untaken.
        assert_eq!(waited, Duration::from_secs(80));
        assert_eq!(started.elapsed(), Duration::from_secs(80));
        assert_eq!(chat.reads_of("C1"), 3);
        assert_eq!(report.outcomes[0].diagnosis, Diagnosis::RateLimited);
        assert!(report.render_for("Slack").contains("rate-limited"));
        assert_eq!(status.phase(), StartupPhase::Failed);
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_named_time_it_backs_off_and_still_stops_at_the_budget() {
        let chat = Scripted::new(|_, _| Err(limited(None)));
        let status = StartupStatus::default();
        let started = tokio::time::Instant::now();
        let checked = patiently(&chat, &[info("C1", "team", true)], patient(30), &status).await;
        let StartupCheck::RateLimited { waited, .. } = checked else {
            panic!("expected the probe to give up on the rate limit, got {checked:?}");
        };
        // Its own backoff is cut short to make one last read exactly at the end of the budget.
        assert_eq!(waited, Duration::from_secs(30));
        assert_eq!(started.elapsed(), Duration::from_secs(30));
        assert!(chat.reads_of("C1") >= 3, "{}", chat.reads_of("C1"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejected_token_still_refuses_at_once() {
        let chat = Scripted::new(|_, _| Err(status(401, r#"{"code":0}"#)));
        let status = StartupStatus::new(StartupPhase::Probing);
        let started = tokio::time::Instant::now();
        let checked = patiently(
            &chat,
            &[info("C1", "a", false), info("C2", "b", false)],
            patient(300),
            &status,
        )
        .await;
        let StartupCheck::Failed(report) = checked else {
            panic!("a rejected token must refuse, got {checked:?}");
        };
        assert!(report.aborted);
        assert_eq!(started.elapsed(), Duration::ZERO, "nothing is waited out");
        assert_eq!(chat.reads_of("C1") + chat.reads_of("C2"), 1);
        assert_eq!(status.phase(), StartupPhase::Failed);
    }

    #[tokio::test(start_paused = true)]
    async fn a_definite_failure_beside_a_rate_limit_refuses_at_once() {
        let chat = Scripted::new(|channel, _| match channel.0.as_str() {
            "C1" => Err(limited(Some(Duration::from_secs(5)))),
            _ => Err(status(404, r#"{"code":10003}"#)),
        });
        let started = tokio::time::Instant::now();
        let checked = patiently(
            &chat,
            &[info("C1", "limited", false), info("C2", "typo'd", false)],
            patient(300),
            &StartupStatus::default(),
        )
        .await;
        assert!(matches!(checked, StartupCheck::Failed(_)), "{checked:?}");
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn only_the_rate_limited_channels_are_read_again() {
        let chat = Scripted::new(|channel, before| match (channel.0.as_str(), before) {
            ("C2", 0) => Err(limited(Some(Duration::from_secs(3)))),
            _ => Ok(Vec::new()),
        });
        let checked = patiently(
            &chat,
            &[info("C1", "quiet", false), info("C2", "busy", false)],
            patient(300),
            &StartupStatus::default(),
        )
        .await;
        let StartupCheck::Passed(report) = checked else {
            panic!("expected a pass, got {checked:?}");
        };
        assert_eq!(report.outcomes.len(), 2);
        assert_eq!(
            chat.reads_of("C1"),
            1,
            "a readable channel is not read again"
        );
        assert_eq!(chat.reads_of("C2"), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_budget_gives_up_at_the_first_rate_limit() {
        let chat = Scripted::new(|_, _| Err(limited(Some(Duration::from_secs(1)))));
        let checked = startup_check(&chat, &[info("C1", "team", true)], false, None).await;
        assert!(
            matches!(checked, StartupCheck::RateLimited { waited, .. } if waited.is_zero()),
            "{checked:?}"
        );
        assert_eq!(chat.reads_of("C1"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_status_says_when_the_next_read_is_due_while_it_waits() {
        let chat = Arc::new(Scripted::new(|_, before| {
            if before == 0 {
                Err(limited(Some(Duration::from_secs(12))))
            } else {
                Ok(Vec::new())
            }
        }));
        let status = Arc::new(StartupStatus::new(StartupPhase::Probing));
        let task = {
            let (chat, status) = (Arc::clone(&chat), Arc::clone(&status));
            tokio::spawn(async move {
                patiently(
                    chat.as_ref(),
                    &[info("C1", "team", true)],
                    patient(300),
                    &status,
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let phase = status.phase();
        assert!(
            matches!(phase, StartupPhase::RateLimited { .. }),
            "{phase:?}"
        );
        assert!(!phase.is_ready() && !phase.is_settled());
        assert_eq!(phase.status(), "starting");
        assert_eq!(
            phase.describe(),
            "probe pending (rate limited, retry in 11s)",
            "12 s asked, 1.5 s gone, rounded up"
        );
        let waiter = {
            let status = Arc::clone(&status);
            tokio::spawn(async move { status.settled().await })
        };
        assert!(matches!(
            task.await.expect("probe"),
            StartupCheck::Passed(_)
        ));
        waiter.await.expect("settled returns once the probe passes");
        assert_eq!(status.phase().status(), "ok");
        assert_eq!(status.phase().describe(), "passed");
    }

    #[test]
    fn the_wait_honours_the_provider_and_otherwise_backs_off_with_bounded_jitter() {
        let patience = patient(300);
        assert_eq!(
            patience.next_wait(Some(Duration::from_secs(9)), 5, 0.0),
            Duration::from_secs(9),
            "a time the provider names is waited exactly"
        );
        // A zero the provider names is no time at all: back off instead of spinning.
        assert_eq!(
            patience.next_wait(Some(Duration::ZERO), 0, 1.0),
            Duration::from_secs(2)
        );
        assert_eq!(patience.next_wait(None, 0, 1.0), Duration::from_secs(2));
        assert_eq!(patience.next_wait(None, 0, 0.0), Duration::from_secs(1));
        assert_eq!(patience.next_wait(None, 3, 1.0), Duration::from_secs(16));
        assert_eq!(patience.next_wait(None, 10, 1.0), Duration::from_secs(60));
        assert_eq!(patience.next_wait(None, 40, 0.0), Duration::from_secs(30));
        for _ in 0..100 {
            let wait = patience.next_wait(None, 2, jitter());
            assert!(
                (Duration::from_secs(4)..=Duration::from_secs(8)).contains(&wait),
                "{wait:?}"
            );
        }
    }

    #[test]
    fn the_wait_budget_comes_from_the_flag_then_the_environment_then_five_minutes() {
        assert_eq!(wait_budget(None, None), Ok(Duration::from_secs(300)));
        assert_eq!(wait_budget(None, Some("")), Ok(Duration::from_secs(300)));
        assert_eq!(wait_budget(None, Some("45")), Ok(Duration::from_secs(45)));
        assert_eq!(wait_budget(Some("0"), Some("45")), Ok(Duration::ZERO));
        let refused = wait_budget(None, Some("5m")).expect_err("not seconds");
        assert!(refused.contains(ENV_STARTUP_PROBE_WAIT), "{refused}");
        let refused = wait_budget(Some("-1"), None).expect_err("not seconds");
        assert!(refused.contains(WAIT_FLAG), "{refused}");
        assert_eq!(wait_budget(Some("86400"), None), Ok(MAX_RATE_LIMIT_WAIT));
        // A deadline this far out would not fit in a clock reading, and would panic the probe.
        assert!(wait_budget(None, Some("18446744073709551615")).is_err());
        assert!(wait_budget(Some("86401"), None).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_patient_probe_asks_the_client_to_leave_the_waiting_to_it() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let chat = {
            let seen = Arc::clone(&seen);
            Scripted::new(move |_, before| {
                seen.lock()
                    .expect("seen")
                    .push(crate::read_reason::hurried());
                if before == 0 {
                    Err(limited(Some(Duration::from_secs(2))))
                } else {
                    Ok(Vec::new())
                }
            })
        };
        let checked = patiently(
            &chat,
            &[info("C1", "team", true)],
            patient(60),
            &StartupStatus::default(),
        )
        .await;
        assert!(matches!(checked, StartupCheck::Passed(_)), "{checked:?}");
        assert_eq!(*seen.lock().expect("seen"), vec![true, true]);

        // With no time to wait, the client keeps its own patience, as before the probe waited.
        seen.lock().expect("seen").clear();
        let _ = startup_check(&chat, &[info("C2", "team", true)], false, None).await;
        assert_eq!(*seen.lock().expect("seen"), vec![false]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_retry_after_is_no_time_named_so_the_last_backoff_is_cut_short() {
        let chat = Scripted::new(|_, _| Err(limited(Some(Duration::ZERO))));
        let started = tokio::time::Instant::now();
        let checked = patiently(
            &chat,
            &[info("C1", "team", true)],
            patient(10),
            &StartupStatus::default(),
        )
        .await;
        let StartupCheck::RateLimited { waited, .. } = checked else {
            panic!("expected the probe to give up on the rate limit, got {checked:?}");
        };
        assert_eq!(waited, Duration::from_secs(10));
        assert_eq!(started.elapsed(), Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_server_with_no_probe_is_ready_and_settled() {
        let status = StartupStatus::default();
        status.settled().await;
        assert!(status.phase().is_ready());
        assert_eq!(status.phase().status(), "ok");
    }
}
