//! Whether the server may go on holding what it read from a provider: the access signals of
//! `#208 persistent-cache`, classified, and the watch that acts on them.
//!
//! # Classifying a failure
//!
//! A provider says "you may not read this" only through a failed operation: there is no event for
//! it. [`classify`] reads one failure ([`ChatError::cause`]) together with WHICH operation failed,
//! and in the [`Dialect`] of the provider it failed at, which its configured kind says (never the
//! failure's body):
//!
//! - **any 401** is the credential refused, whatever the operation; Slack's `account_inactive` and
//!   `token_revoked` say it is gone for good;
//! - **a 403 or 404 from a read of a channel's messages** ([`CHANNEL_READS`]) may be access to the
//!   channel lost, or to the one thread the read was of. On Slack only `channel_not_found` and
//!   `not_in_channel` (the channel) and `thread_not_found` (the thread read) count: a 403 or 404
//!   that carries no Slack code at all was not Slack's answer but that of something in between (a
//!   shim route that is gone, an intermediary refusing the destination, a wrong `api_base`), and
//!   says nothing about access;
//! - **everything else is excluded**: never a block, never an erase. That is every post, a single
//!   message's lookup, a read cursor moved or cleared (whose 404 means the provider no longer holds
//!   that one message), the identity and directory calls, any 400, Slack's 403 or 404 with no code,
//!   and Slack's `message_not_found`, `user_not_found`, `missing_scope`, `not_allowed_token_type`,
//!   `restricted_action`, `is_archived`, `access_denied` and `no_permission`.
//!
//! The Slack client writes the code into the failure's body as `"{method} answered {code}"`; a
//! unit test there pins that shape against [`slack_code`]. Only a Slack provider's failures are
//! read for one.
//!
//! # The watch
//!
//! Every provider's client inside a [`crate::providers::ChatRouter`] reports each operation's
//! outcome to the router's one [`AccessWatch`] ([`crate::upstream_profile::ProfiledClient`]), so a
//! signal is heard whoever made the read: a request at any scope, or the server's own poller. The
//! watch does nothing at all until something listens ([`AccessWatch::listen`]): the message
//! cache's sink, and only while a cache is behind it.
//!
//! A suspected loss is acted on at once by blocking what the cache would serve of that channel or
//! thread scope, in memory, and confirmed only by the server: [`CONFIRM_AFTER`] after the first
//! signal, the watch has its listener read the scope again on the server's own account. The same
//! signal confirms the loss, and the listener erases what it held; a successful read of the scope
//! at any moment before then clears the doubt and the block; any other answer leaves the doubt and
//! asks again later. The wait is there because a provider bridge can turn any error text that
//! mentions "not found" into a not-found code, so one refusal proves nothing.
//!
//! A credential refused is handed straight to the listener, which suspends the provider.
//!
//! An erase is the one durable change a read-scope request can set off, and only after the
//! server's own read confirmed the loss. It removes what was read and writes no text: a channel's
//! erase leaves nothing, and a thread's leaves only the ids and times of the messages it deleted, as
//! the tombstones that keep a read landing late from bringing them back.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;

use crate::chat::ChatError;
use crate::model::ChannelId;

/// How long after the first refusal of a channel or thread the server reads it again to confirm
/// the loss.
pub const CONFIRM_AFTER: Duration = Duration::from_secs(600);

/// The operations that read a channel's messages, or one thread's: the only ones whose 403 or 404
/// can mean access to the channel or the thread is lost.
pub const CHANNEL_READS: [&str; 6] = [
    "fetch_timeline",
    "fetch_timeline_since",
    "fetch_page",
    "fetch_recent",
    "fetch_recent_dated",
    "probe_channel",
];

/// Slack codes for a credential that is gone for good, not merely refused this once.
const REVOKED: [&str; 2] = ["account_inactive", "token_revoked"];
/// Slack codes for access to a channel lost.
const CHANNEL_LOST: [&str; 2] = ["channel_not_found", "not_in_channel"];
/// The Slack code for a thread that cannot be read.
const THREAD_LOST: &str = "thread_not_found";

/// How a provider says what it refused: what its failures mean depends on what kind of provider it
/// is, which its configuration says ([`Dialect::of`], [`AccessWatch::set_dialect`]). Never guessed
/// from a failure's body: a page in front of Slack can say anything, and a Discord-protocol body
/// can look like a Slack one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    /// Discord, or a bridge speaking its API: an HTTP 403 or 404 is the provider's own answer
    /// about what was read.
    #[default]
    DiscordProtocol,
    /// Slack's Web API, or a bridge speaking its method subset: the provider answers in codes,
    /// which its client writes into the failure's body ([`slack_code`]). A 403 or 404 that carries
    /// none was answered by something in between, and says nothing about access.
    Slack,
}

impl Dialect {
    /// The dialect of a provider configured as `kind`.
    #[must_use]
    pub fn of(kind: &crate::config::ProviderKind) -> Self {
        match kind {
            crate::config::ProviderKind::Discord(_) => Self::DiscordProtocol,
            crate::config::ProviderKind::Slack(_) => Self::Slack,
        }
    }
}

/// What one failed operation says about the server's right to hold what it read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The provider refused the credential (a 401). `revoked` when it said the account or the
    /// token is gone for good.
    CredentialRefused {
        /// Whether the credential is gone for good.
        revoked: bool,
    },
    /// A read of a channel's messages was refused or found nothing: access to the channel, or to
    /// the thread `thread` alone, may be lost.
    ChannelLost {
        /// The thread whose scope alone is in doubt; `None` for the whole channel.
        thread: Option<String>,
    },
    /// Nothing about access.
    Excluded,
}

/// What `error`, from the operation `op` on the thread `thread` (when it read one) of a provider
/// that speaks `dialect`, says about access. See the module documentation.
#[must_use]
pub fn classify(dialect: Dialect, op: &str, thread: Option<&str>, error: &ChatError) -> Signal {
    let ChatError::Status { status, body } = error.cause() else {
        return Signal::Excluded;
    };
    let channel_read = CHANNEL_READS.contains(&op);
    match dialect {
        Dialect::DiscordProtocol => match status {
            401 => Signal::CredentialRefused { revoked: false },
            // The provider's own refusal of a channel read, of the thread it read.
            403 | 404 if channel_read => Signal::ChannelLost {
                thread: thread.map(str::to_owned),
            },
            _ => Signal::Excluded,
        },
        Dialect::Slack => {
            let code = slack_code(body);
            match status {
                401 => Signal::CredentialRefused {
                    revoked: code.is_some_and(|code| REVOKED.contains(&code)),
                },
                403 | 404 if channel_read => match code {
                    Some(code) if CHANNEL_LOST.contains(&code) => {
                        Signal::ChannelLost { thread: None }
                    }
                    Some(THREAD_LOST) if thread.is_some() => Signal::ChannelLost {
                        thread: thread.map(str::to_owned),
                    },
                    // Any other code says something other than lost access; no code at all says
                    // Slack did not answer.
                    Some(_) | None => Signal::Excluded,
                },
                _ => Signal::Excluded,
            }
        }
    }
}

/// The Slack `error` code in a failure's body, when the body has the shape the Slack client gives
/// every `{ok:false}` answer: `"{method} answered {code}"`, perhaps with more after the code.
/// `None` for any other body, such as a Discord-protocol provider's JSON.
#[must_use]
pub fn slack_code(body: &str) -> Option<&str> {
    let (method, rest) = body.split_once(" answered ")?;
    let is_method = method.contains('.')
        && method
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
    if !is_method {
        return None;
    }
    let end = rest
        .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
        .unwrap_or(rest.len());
    let code = &rest[..end];
    (!code.is_empty()).then_some(code)
}

/// A channel of a provider, or one thread of it, whose access is in doubt.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Suspect {
    /// The provider that refused.
    pub provider_key: String,
    /// The channel.
    pub channel: ChannelId,
    /// The thread whose scope alone is in doubt; `None` for the whole channel.
    pub thread: Option<String>,
}

impl Suspect {
    /// The operation the server's own confirming read makes: the channel's sample, or a page of
    /// the thread.
    #[must_use]
    pub fn probe_op(&self) -> &'static str {
        if self.thread.is_some() {
            "fetch_timeline"
        } else {
            "probe_channel"
        }
    }
}

/// What acts on what the watch hears: the message cache's sink.
#[async_trait]
pub trait AccessListener: Send + Sync {
    /// Stop serving `suspect`'s scope from the cache.
    fn block(&self, suspect: &Suspect);
    /// Serve it again: a read of it succeeded.
    fn unblock(&self, suspect: &Suspect);
    /// Read `suspect`'s scope again, on the server's own account, through the provider's client.
    /// `None` when there is nothing to read it with, or nothing to confirm for.
    async fn probe(&self, suspect: &Suspect) -> Option<Result<(), ChatError>>;
    /// The server's own read confirmed the loss: erase what is held of the scope.
    fn confirmed(&self, suspect: &Suspect);
    /// `provider_key` refused the credential; `revoked` when for good.
    fn credential_refused(&self, provider_key: &str, revoked: bool);
}

/// One doubt: when it began, and which one it is, so that a confirmation scheduled for a doubt
/// already cleared does not act on a later one.
#[derive(Clone, Copy, Debug)]
struct Doubt {
    id: u64,
    since: tokio::time::Instant,
}

/// Hears every provider operation's outcome, and keeps the doubts about access they raise. One per
/// [`crate::providers::ChatRouter`]. See the module documentation.
#[derive(Default)]
pub struct AccessWatch {
    inner: Arc<Watch>,
}

#[derive(Default)]
struct Watch {
    listener: RwLock<Option<Weak<dyn AccessListener>>>,
    doubts: Mutex<HashMap<Suspect, Doubt>>,
    next: AtomicU64,
    /// Each provider's dialect, by key, as its configuration says.
    dialects: RwLock<HashMap<String, Dialect>>,
}

impl std::fmt::Debug for AccessWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessWatch")
            .field("doubts", &self.inner.doubts().len())
            .finish_non_exhaustive()
    }
}

impl AccessWatch {
    /// A watch nothing listens to: it hears and does nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Have `listener` act on what the watch hears from now on, in place of any other; `None`
    /// stops it. Held weakly: a listener that is gone hears nothing.
    pub fn listen(&self, listener: Option<Weak<dyn AccessListener>>) {
        *self
            .inner
            .listener
            .write()
            .unwrap_or_else(PoisonError::into_inner) = listener;
    }

    /// Read `provider_key`'s failures as `dialect` from now on: what its configured kind says
    /// ([`Dialect::of`]). [`crate::providers::ChatRouter::from_config`] sets every provider's. A
    /// provider never set is read as a Discord-protocol one, as every router built without a
    /// configuration is.
    pub fn set_dialect(&self, provider_key: &str, dialect: Dialect) {
        self.inner
            .dialects
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(provider_key.to_owned(), dialect);
    }

    /// The dialect `provider_key`'s failures are read in.
    #[must_use]
    pub fn dialect(&self, provider_key: &str) -> Dialect {
        self.inner.dialect(provider_key)
    }

    /// Every scope in doubt now: for a listener that must block afresh what it no longer holds
    /// blocked, such as a cache opened since.
    #[must_use]
    pub fn suspects(&self) -> Vec<Suspect> {
        self.inner.doubts().keys().cloned().collect()
    }

    /// Hear how one operation of `provider_key` ended: `op`, the method's name, on `channel`, and
    /// in `thread`'s scope when the read was of one thread. Does nothing while nothing listens.
    pub fn report(
        &self,
        provider_key: &str,
        op: &'static str,
        channel: Option<&ChannelId>,
        thread: Option<&str>,
        outcome: Result<(), &ChatError>,
    ) {
        let Some(listener) = self.inner.listener() else {
            return;
        };
        match outcome {
            Ok(()) => {
                let Some(channel) = channel.filter(|_| CHANNEL_READS.contains(&op)) else {
                    return;
                };
                // A read of the channel clears the doubt about the channel; a read of one of its
                // threads, about that thread and about the channel too.
                let suspect = |thread: Option<&str>| Suspect {
                    provider_key: provider_key.to_owned(),
                    channel: channel.clone(),
                    thread: thread.map(str::to_owned),
                };
                let mut cleared = Vec::new();
                {
                    let mut doubts = self.inner.doubts();
                    for candidate in [Some(suspect(None)), thread.map(|t| suspect(Some(t)))]
                        .into_iter()
                        .flatten()
                    {
                        if doubts.remove(&candidate).is_some() {
                            cleared.push(candidate);
                        }
                    }
                }
                for suspect in &cleared {
                    listener.unblock(suspect);
                }
            }
            Err(error) => match classify(self.inner.dialect(provider_key), op, thread, error) {
                Signal::Excluded => {}
                Signal::CredentialRefused { revoked } => {
                    listener.credential_refused(provider_key, revoked);
                }
                Signal::ChannelLost { thread } => {
                    let Some(channel) = channel else {
                        return;
                    };
                    let suspect = Suspect {
                        provider_key: provider_key.to_owned(),
                        channel: channel.clone(),
                        thread,
                    };
                    self.suspect(&listener, suspect);
                }
            },
        }
    }

    /// Doubt `suspect`, unless it is doubted already: block it, and have the server confirm it.
    fn suspect(&self, listener: &Arc<dyn AccessListener>, suspect: Suspect) {
        let doubt = {
            let mut doubts = self.inner.doubts();
            if doubts.contains_key(&suspect) {
                return;
            }
            let doubt = Doubt {
                id: self.inner.next.fetch_add(1, Ordering::Relaxed),
                since: tokio::time::Instant::now(),
            };
            doubts.insert(suspect.clone(), doubt);
            doubt
        };
        listener.block(&suspect);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(confirm(Arc::downgrade(&self.inner), suspect, doubt));
        }
    }
}

impl Watch {
    fn dialect(&self, provider_key: &str) -> Dialect {
        self.dialects
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(provider_key)
            .copied()
            .unwrap_or_default()
    }

    fn listener(&self) -> Option<Arc<dyn AccessListener>> {
        self.listener
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    fn doubts(&self) -> std::sync::MutexGuard<'_, HashMap<Suspect, Doubt>> {
        self.doubts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `doubt` about `suspect` still stands.
    fn stands(&self, suspect: &Suspect, doubt: Doubt) -> bool {
        self.doubts()
            .get(suspect)
            .is_some_and(|held| held.id == doubt.id)
    }

    /// End `doubt` about `suspect`, if it still stands. Whether it did.
    fn end(&self, suspect: &Suspect, doubt: Doubt) -> bool {
        let mut doubts = self.doubts();
        if doubts.get(suspect).is_some_and(|held| held.id == doubt.id) {
            doubts.remove(suspect);
            true
        } else {
            false
        }
    }
}

/// The server's confirmation of one doubt: [`CONFIRM_AFTER`] after it began, read the scope again
/// through the listener; the same signal confirms it, a success clears it, anything else asks again
/// [`CONFIRM_AFTER`] later. Ends as soon as the doubt is cleared some other way.
async fn confirm(watch: Weak<Watch>, suspect: Suspect, doubt: Doubt) {
    let lost = Signal::ChannelLost {
        thread: suspect.thread.clone(),
    };
    let mut at = doubt.since + CONFIRM_AFTER;
    loop {
        tokio::time::sleep_until(at).await;
        let Some(listener) = watch
            .upgrade()
            .filter(|watch| watch.stands(&suspect, doubt))
            .and_then(|watch| watch.listener())
        else {
            return;
        };
        let answer = listener.probe(&suspect).await;
        let Some(watch) = watch.upgrade() else {
            return;
        };
        match answer {
            Some(Err(error))
                if classify(
                    watch.dialect(&suspect.provider_key),
                    suspect.probe_op(),
                    suspect.thread.as_deref(),
                    &error,
                ) == lost =>
            {
                if watch.end(&suspect, doubt) {
                    listener.confirmed(&suspect);
                }
                return;
            }
            Some(Ok(())) | None => {
                if watch.end(&suspect, doubt) {
                    listener.unblock(&suspect);
                }
                return;
            }
            Some(Err(_)) => at = tokio::time::Instant::now() + CONFIRM_AFTER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(status: u16, body: &str) -> ChatError {
        ChatError::Status {
            status,
            body: body.to_owned(),
        }
    }

    fn lost(thread: Option<&str>) -> Signal {
        Signal::ChannelLost {
            thread: thread.map(str::to_owned),
        }
    }

    /// A failure of a Discord-protocol provider, read as one.
    fn discord(op: &str, thread: Option<&str>, error: &ChatError) -> Signal {
        classify(Dialect::DiscordProtocol, op, thread, error)
    }

    /// A failure of a Slack provider, read as one.
    fn slack(op: &str, thread: Option<&str>, error: &ChatError) -> Signal {
        classify(Dialect::Slack, op, thread, error)
    }

    /// A failure as the Slack client writes one: Slack's code in the body.
    fn coded(status_code: u16, method: &str, code: &str) -> ChatError {
        status(status_code, &format!("{method} answered {code}")).with_provider("Slack")
    }

    #[test]
    fn a_dialect_is_what_the_providers_configured_kind_says() {
        let text = r#"
[auth]
read_token = "read-token-that-is-long-enough"
write_token = "write-token-that-is-long-enough"

[[providers]]
key = "chat"
kind = "discord"
api_base = "http://bridge.invalid/api/v10"
token = "bridge-token"

[[providers]]
key = "team"
kind = "slack"
token = "xoxb-slack-token"

[[channels]]
id = "900000000000000201"
label = "home"
provider = "chat"

[[channels]]
id = "C0123ABCDE"
label = "team"
provider = "team"
"#;
        let config =
            crate::config::Config::from_toml_and_env(text, &std::collections::BTreeMap::new())
                .expect("a configuration");
        let kind = |key: &str| &config.provider(key).expect("configured").kind;
        assert_eq!(Dialect::of(kind("chat")), Dialect::DiscordProtocol);
        assert_eq!(Dialect::of(kind("team")), Dialect::Slack);
        // The router built from it reads each provider's failures in its own dialect.
        let router = crate::providers::ChatRouter::from_config(&config).expect("clients");
        assert_eq!(router.access().dialect("chat"), Dialect::DiscordProtocol);
        assert_eq!(router.access().dialect("team"), Dialect::Slack);
        assert_eq!(
            AccessWatch::new().dialect("unset"),
            Dialect::DiscordProtocol,
            "a router built without a configuration"
        );
    }

    #[test]
    fn every_401_refuses_the_credential_and_only_two_slack_codes_say_for_good() {
        for op in [
            "post_message",
            "identity",
            "fetch_timeline",
            "mark_read_upstream",
        ] {
            let error = status(401, "{\"message\": \"401: Unauthorized\"}");
            for read in [discord, slack] {
                assert_eq!(
                    read(op, None, &error),
                    Signal::CredentialRefused { revoked: false },
                    "{op}"
                );
            }
        }
        for (code, revoked) in [
            ("invalid_auth", false),
            ("not_authed", false),
            ("token_expired", false),
            ("account_inactive", true),
            ("token_revoked", true),
        ] {
            let error = coded(401, "auth.test", code);
            assert_eq!(
                slack("identity", None, &error),
                Signal::CredentialRefused { revoked },
                "{code}"
            );
            // A Discord-protocol provider's body is never read for a Slack code.
            assert_eq!(
                discord("identity", None, &error),
                Signal::CredentialRefused { revoked: false },
                "{code}"
            );
        }
    }

    #[test]
    fn a_403_or_404_counts_only_from_a_read_of_a_channels_messages() {
        let refused = status(403, "{\"message\": \"Missing Access\", \"code\": 50001}");
        let missing = status(404, "{\"message\": \"Unknown Channel\", \"code\": 10003}");
        for op in CHANNEL_READS {
            assert_eq!(discord(op, None, &refused), lost(None), "{op}");
            assert_eq!(discord(op, None, &missing), lost(None), "{op}");
        }
        assert_eq!(
            discord("fetch_timeline", Some("77"), &missing),
            lost(Some("77")),
            "a provider's refusal of a thread read is about that thread"
        );
        for op in [
            "post_message",
            "post_message_keyed",
            "post_in_thread",
            "post_in_thread_keyed",
            "fetch_thread_message",
            "mark_read_upstream",
            "clear_upstream_through",
            "identity",
            "discover_channels",
            "register_channel",
            "unregister_channel",
        ] {
            for read in [discord, slack] {
                assert_eq!(read(op, None, &refused), Signal::Excluded, "{op}");
                assert_eq!(read(op, Some("77"), &missing), Signal::Excluded, "{op}");
                assert_eq!(
                    read(
                        op,
                        None,
                        &coded(404, "conversations.info", "channel_not_found")
                    ),
                    Signal::Excluded,
                    "{op}"
                );
            }
        }
    }

    /// A Discord-protocol provider's 403 or 404 is its own answer, whatever its body looks like:
    /// the body is never read for a Slack code, which a bridge's error text may happen to mimic.
    #[test]
    fn a_discord_protocol_refusal_counts_whatever_its_body_looks_like() {
        for code in ["missing_scope", "message_not_found", "channel_not_found"] {
            assert_eq!(
                discord(
                    "fetch_timeline",
                    None,
                    &coded(404, "conversations.history", code)
                ),
                lost(None),
                "{code}"
            );
        }
        assert_eq!(
            discord(
                "fetch_timeline",
                Some("77"),
                &coded(404, "conversations.history", "channel_not_found")
            ),
            lost(Some("77")),
            "nor does a code in its body widen a thread's doubt to the channel"
        );
    }

    #[test]
    fn on_slack_only_three_codes_say_access_is_lost_and_a_thread_code_only_for_a_thread() {
        assert_eq!(
            slack(
                "fetch_timeline",
                None,
                &coded(404, "conversations.history", "channel_not_found")
            ),
            lost(None)
        );
        assert_eq!(
            slack(
                "fetch_recent",
                None,
                &coded(403, "conversations.history", "not_in_channel")
            ),
            lost(None)
        );
        assert_eq!(
            slack(
                "fetch_timeline",
                Some("1700000000.000100"),
                &coded(404, "conversations.replies", "thread_not_found")
            ),
            lost(Some("1700000000.000100"))
        );
        assert_eq!(
            slack(
                "fetch_timeline",
                Some("1700000000.000100"),
                &coded(404, "conversations.replies", "channel_not_found")
            ),
            lost(None),
            "a channel code from a thread read is about the channel"
        );
        assert_eq!(
            slack(
                "fetch_timeline",
                None,
                &coded(404, "conversations.replies", "thread_not_found")
            ),
            Signal::Excluded,
            "a thread code from a read of no one thread names no scope"
        );
        for (status_code, code) in [
            (404, "message_not_found"),
            (404, "user_not_found"),
            (403, "missing_scope"),
            (403, "not_allowed_token_type"),
            (403, "restricted_action"),
            (403, "is_archived"),
            (403, "access_denied"),
            (403, "no_permission"),
            (403, "team_access_not_granted"),
        ] {
            assert_eq!(
                slack(
                    "fetch_timeline",
                    None,
                    &coded(status_code, "conversations.history", code)
                ),
                Signal::Excluded,
                "{code}"
            );
        }
        let scoped = status(
            403,
            "conversations.history answered missing_scope (needed scope: channels:history)",
        );
        assert_eq!(slack("fetch_page", None, &scoped), Signal::Excluded);
    }

    /// Plan section 6.3: on Slack only the three codes count. An HTTP 403 or 404 that carries no
    /// Slack code was answered by something in front of Slack: a shim route that is gone or
    /// misrouted, an intermediary refusing the destination, a wrong `api_base`, a shim answering
    /// its own failure. From every read of a channel's messages, of the channel or of one thread,
    /// it is never a doubt about access, so never a block and never an erase.
    #[test]
    fn on_slack_a_403_or_404_with_no_slack_code_says_nothing_about_access() {
        for error in [
            status(404, "<html><body><h1>404 Not Found</h1></body></html>"),
            status(403, "Forbidden: destination not allowed"),
            status(404, ""),
            status(403, "{\"message\": \"Missing Access\", \"code\": 50001}"),
            status(404, "channel_not_found"),
            status(404, "upstream answered channel_not_found"),
            status(404, "<html>404</html>").with_provider("Slack"),
        ] {
            for op in CHANNEL_READS {
                for thread in [None, Some("1700000000.000100")] {
                    assert_eq!(
                        slack(op, thread, &error),
                        Signal::Excluded,
                        "{op} {thread:?}: {error}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_400_and_every_failure_that_is_not_a_refusal_are_excluded() {
        for error in [
            status(400, "conversations.history answered invalid_cursor"),
            status(400, "{\"message\": \"Invalid Form Body\"}"),
            status(409, "{\"error\": \"not a space\"}"),
            status(500, "channel_not_found"),
            status(500, "conversations.history answered channel_not_found"),
            status(503, "unavailable"),
            ChatError::Transport("reset".to_owned()),
            ChatError::Shape("odd".to_owned()),
            ChatError::Refused("no".to_owned()),
            ChatError::CursorExpired("old".to_owned()),
        ] {
            for read in [discord, slack] {
                assert_eq!(
                    read("fetch_timeline", None, &error),
                    Signal::Excluded,
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn a_slack_code_is_read_only_out_of_the_shape_the_slack_client_writes() {
        assert_eq!(
            slack_code("conversations.history answered channel_not_found"),
            Some("channel_not_found")
        );
        assert_eq!(
            slack_code("conversations.history answered missing_scope (needed scope: x:y)"),
            Some("missing_scope")
        );
        assert_eq!(
            slack_code("auth.test answered token_revoked"),
            Some("token_revoked")
        );
        for body in [
            "",
            "channel_not_found",
            "{\"message\": \"Unknown Channel\", \"code\": 10003}",
            "upstream answered channel_not_found",
            "conversations.history answered ",
            "conversations.history answered (oops)",
            "a b.c answered channel_not_found",
        ] {
            assert_eq!(slack_code(body), None, "{body:?}");
        }
    }

    /// A listener that writes down what it is told, and answers each confirming read as told.
    #[derive(Default)]
    struct Notes {
        heard: Mutex<Vec<String>>,
        answers: Mutex<Vec<Option<Result<(), ChatError>>>>,
    }

    impl Notes {
        fn note(&self, what: String) {
            self.heard.lock().expect("notes").push(what);
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.heard.lock().expect("notes"))
        }

        fn answer_next(&self, answer: Option<Result<(), ChatError>>) {
            self.answers.lock().expect("answers").push(answer);
        }
    }

    fn name(suspect: &Suspect) -> String {
        match &suspect.thread {
            Some(thread) => format!("{}/{}#{thread}", suspect.provider_key, suspect.channel),
            None => format!("{}/{}", suspect.provider_key, suspect.channel),
        }
    }

    #[async_trait]
    impl AccessListener for Notes {
        fn block(&self, suspect: &Suspect) {
            self.note(format!("block {}", name(suspect)));
        }
        fn unblock(&self, suspect: &Suspect) {
            self.note(format!("unblock {}", name(suspect)));
        }
        async fn probe(&self, suspect: &Suspect) -> Option<Result<(), ChatError>> {
            self.note(format!("probe {}", name(suspect)));
            let mut answers = self.answers.lock().expect("answers");
            if answers.is_empty() {
                None
            } else {
                answers.remove(0)
            }
        }
        fn confirmed(&self, suspect: &Suspect) {
            self.note(format!("erase {}", name(suspect)));
        }
        fn credential_refused(&self, provider_key: &str, revoked: bool) {
            self.note(format!("refused {provider_key} revoked={revoked}"));
        }
    }

    fn listening() -> (AccessWatch, Arc<Notes>) {
        let watch = AccessWatch::new();
        let notes = Arc::new(Notes::default());
        let listener: Arc<dyn AccessListener> = notes.clone();
        watch.listen(Some(Arc::downgrade(&listener)));
        (watch, notes)
    }

    fn channel() -> ChannelId {
        ChannelId("C1".to_owned())
    }

    fn missing() -> ChatError {
        status(404, "{\"message\": \"Unknown Channel\", \"code\": 10003}")
    }

    /// A little past a deadline: the timer rounds a deadline up to its next millisecond.
    const PAST: Duration = Duration::from_millis(5);

    async fn run_tasks() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn nothing_is_done_while_nothing_listens() {
        let watch = AccessWatch::new();
        let missing = missing();
        watch.report("p", "fetch_timeline", Some(&channel()), None, Err(&missing));
        assert!(watch.suspects().is_empty());
        // A listener that stopped listening hears nothing either.
        let (watch, notes) = listening();
        watch.listen(None);
        watch.report("p", "fetch_timeline", Some(&channel()), None, Err(&missing));
        assert!(notes.take().is_empty());
        assert!(watch.suspects().is_empty());
    }

    #[tokio::test]
    async fn one_refusal_blocks_and_the_servers_read_ten_minutes_later_confirms_it() {
        tokio::time::pause();
        let (watch, notes) = listening();
        let missing = missing();
        watch.report("p", "fetch_timeline", Some(&channel()), None, Err(&missing));
        // A second refusal before the confirmation changes nothing: the doubt is the same one.
        watch.report("p", "fetch_page", Some(&channel()), None, Err(&missing));
        assert_eq!(notes.take(), ["block p/C1"]);
        assert_eq!(watch.suspects().len(), 1);

        tokio::time::advance(CONFIRM_AFTER - Duration::from_secs(1)).await;
        run_tasks().await;
        assert!(notes.take().is_empty(), "not before ten minutes");

        notes.answer_next(Some(Err(missing.clone())));
        tokio::time::advance(Duration::from_secs(1) + PAST).await;
        run_tasks().await;
        assert_eq!(notes.take(), ["probe p/C1", "erase p/C1"]);
        assert!(watch.suspects().is_empty());
    }

    #[tokio::test]
    async fn a_successful_read_in_between_clears_the_doubt_and_no_confirmation_reads() {
        tokio::time::pause();
        let (watch, notes) = listening();
        let missing = missing();
        watch.report("p", "fetch_recent", Some(&channel()), None, Err(&missing));
        // A post succeeding says nothing about reading; a read of the channel does.
        watch.report("p", "post_message", Some(&channel()), None, Ok(()));
        assert_eq!(notes.take(), ["block p/C1"]);
        watch.report("p", "fetch_timeline", Some(&channel()), None, Ok(()));
        assert_eq!(notes.take(), ["unblock p/C1"]);
        tokio::time::advance(CONFIRM_AFTER * 2).await;
        run_tasks().await;
        assert!(notes.take().is_empty(), "no read, no erase");
    }

    #[tokio::test]
    async fn a_thread_is_doubted_alone_and_a_read_of_the_channel_does_not_clear_it() {
        tokio::time::pause();
        let (watch, notes) = listening();
        let missing = missing();
        watch.report(
            "p",
            "fetch_timeline",
            Some(&channel()),
            Some("77"),
            Err(&missing),
        );
        assert_eq!(notes.take(), ["block p/C1#77"]);
        watch.report("p", "fetch_timeline", Some(&channel()), None, Ok(()));
        assert!(
            notes.take().is_empty(),
            "the channel's main read says nothing of the thread"
        );
        notes.answer_next(Some(Err(missing.clone())));
        tokio::time::advance(CONFIRM_AFTER + PAST).await;
        run_tasks().await;
        assert_eq!(notes.take(), ["probe p/C1#77", "erase p/C1#77"]);

        // A thread read succeeding clears a doubt about the thread and about its channel.
        watch.report("p", "fetch_page", Some(&channel()), None, Err(&missing));
        watch.report(
            "p",
            "fetch_timeline",
            Some(&channel()),
            Some("78"),
            Err(&missing),
        );
        assert_eq!(notes.take(), ["block p/C1", "block p/C1#78"]);
        watch.report("p", "fetch_timeline", Some(&channel()), Some("78"), Ok(()));
        let mut cleared = notes.take();
        cleared.sort();
        assert_eq!(cleared, ["unblock p/C1", "unblock p/C1#78"]);
    }

    #[tokio::test]
    async fn a_confirming_read_that_says_something_else_asks_again_later() {
        tokio::time::pause();
        let (watch, notes) = listening();
        let missing = missing();
        watch.report("p", "fetch_timeline", Some(&channel()), None, Err(&missing));
        notes.take();
        notes.answer_next(Some(Err(ChatError::Transport("reset".to_owned()))));
        tokio::time::advance(CONFIRM_AFTER + PAST).await;
        run_tasks().await;
        assert_eq!(
            notes.take(),
            ["probe p/C1"],
            "still blocked, nothing erased"
        );
        assert_eq!(watch.suspects().len(), 1);
        notes.answer_next(Some(Ok(())));
        tokio::time::advance(CONFIRM_AFTER + PAST).await;
        run_tasks().await;
        assert_eq!(notes.take(), ["probe p/C1", "unblock p/C1"]);
        assert!(watch.suspects().is_empty());
    }

    /// A Slack provider's 403 or 404 with no Slack code, from any read of a channel's messages,
    /// is heard and nothing comes of it: no doubt, no block, no confirming read, no erase. And a
    /// confirming read answered that way, of a doubt a real code raised, confirms nothing: it asks
    /// again later, and a success then clears the doubt.
    #[tokio::test]
    async fn a_slack_providers_codeless_refusal_neither_doubts_nor_confirms() {
        tokio::time::pause();
        let (watch, notes) = listening();
        watch.set_dialect("s", Dialect::Slack);
        let page = status(404, "<html><body><h1>404 Not Found</h1></body></html>");
        let refused = status(403, "Forbidden: destination not allowed");
        for op in CHANNEL_READS {
            for error in [&page, &refused] {
                watch.report("s", op, Some(&channel()), None, Err(error));
                watch.report(
                    "s",
                    op,
                    Some(&channel()),
                    Some("1700000000.000100"),
                    Err(error),
                );
            }
        }
        assert!(notes.take().is_empty(), "nothing blocked");
        assert!(watch.suspects().is_empty(), "nothing in doubt");
        tokio::time::advance(CONFIRM_AFTER * 2 + PAST).await;
        run_tasks().await;
        assert!(notes.take().is_empty(), "no confirming read, no erase");

        // A real code raises the doubt; the confirming read meets the shim's own 404.
        let gone = coded(404, "conversations.history", "channel_not_found");
        watch.report("s", "fetch_recent", Some(&channel()), None, Err(&gone));
        assert_eq!(notes.take(), ["block s/C1"]);
        notes.answer_next(Some(Err(page.clone())));
        tokio::time::advance(CONFIRM_AFTER + PAST).await;
        run_tasks().await;
        assert_eq!(
            notes.take(),
            ["probe s/C1"],
            "still blocked, nothing erased"
        );
        assert_eq!(watch.suspects().len(), 1);
        notes.answer_next(Some(Ok(())));
        tokio::time::advance(CONFIRM_AFTER + PAST).await;
        run_tasks().await;
        assert_eq!(notes.take(), ["probe s/C1", "unblock s/C1"]);

        // The same failures from a Discord-protocol provider are its own refusal.
        watch.report("p", "fetch_recent", Some(&channel()), None, Err(&page));
        assert_eq!(notes.take(), ["block p/C1"]);
    }

    #[tokio::test]
    async fn a_refused_credential_goes_straight_to_the_listener_and_exclusions_go_nowhere() {
        let (watch, notes) = listening();
        watch.set_dialect("s", Dialect::Slack);
        let revoked = status(401, "conversations.history answered token_revoked");
        watch.report("s", "post_message", Some(&channel()), None, Err(&revoked));
        assert_eq!(notes.take(), ["refused s revoked=true"]);
        let refused = status(401, "{\"message\": \"401: Unauthorized\"}");
        watch.report("p", "post_message", Some(&channel()), None, Err(&refused));
        assert_eq!(notes.take(), ["refused p revoked=false"]);
        for (provider, op, error) in [
            (
                "p",
                "post_message",
                status(403, "{\"message\": \"Missing Permissions\"}"),
            ),
            (
                "s",
                "fetch_timeline",
                status(404, "conversations.history answered message_not_found"),
            ),
            ("p", "clear_upstream_through", missing()),
            ("p", "fetch_timeline", status(400, "{}")),
            ("s", "fetch_timeline", status(404, "Not Found")),
        ] {
            watch.report(provider, op, Some(&channel()), None, Err(&error));
        }
        assert!(notes.take().is_empty());
        assert!(watch.suspects().is_empty());
    }
}
