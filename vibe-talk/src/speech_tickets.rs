//! Everything a read-aloud needs, resolved BEFORE the reader taps.
//!
//! # Why this exists
//!
//! Reading a message aloud used to do all of its work on the tap: fetch a window of fifty messages
//! from Discord to find one by id, normalise its text for speech, ask ElevenLabs which voice the
//! agent speaks in, and only then start generating audio. Four waits in series, every time, and the
//! reader is sitting there through all of them having already been shown the very text they are
//! waiting to hear.
//!
//! The page already HAS that text — it drew it. So none of that work needs to happen on the tap. It
//! happens once, when read-aloud mode is turned on, and what the tap gets is a TICKET: an opaque
//! string naming one message whose text is already resolved and waiting. Playing it is then a
//! single hop that does a map lookup and starts forwarding bytes.
//!
//! # Why a ticket rather than the text
//!
//! The obvious shortcut is to let the page send the text it already has. That is refused here for
//! the same reason `/speak` always refused it: this server holds the operator's ElevenLabs
//! credential, and a route that reads back whatever it is handed is a route that spends their
//! balance on anything at all. The text in a ticket was resolved by this server FROM A REAL MESSAGE
//! and cannot be edited afterwards, so the guarantee is if anything stronger than before — it is
//! fixed at mint time rather than re-derived under a caller-supplied id.
//!
//! # Why the ticket is a capability
//!
//! It travels as a URL, because that is the whole point: an `<audio src>` cannot carry an
//! `Authorization` header, and without it the browser cannot stream. So the ticket has to
//! authenticate on its own. It is 256 bits from the system CSPRNG, it names exactly one message, it
//! expires in minutes, and the store it lives in is capped. What it grants — hearing one message
//! the holder could already read, plus appending bounded content-free playback timings for that
//! same read — is deliberately the smallest thing that makes the URL work. The timing callback
//! takes no bearer header for exactly the same reason as the audio element: the random ticket is
//! intentionally its capability too. It answers one 404 for a ticket that was never minted, one
//! that has expired, and one whose audio has not been played yet, so it reveals nothing the audio
//! route does not.
//!
//! It WILL appear in the page's DOM and in this server's access log, which is the cost of the
//! approach and the reason for the short life and the narrow scope.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a minted ticket stays playable.
///
/// Long enough to turn the mode on, read a screenful, and tap something near the bottom; short
/// enough that a URL copied out of a log is worthless by the time anyone reads it. A ticket that
/// has expired is not an error the reader should ever see — the page re-mints as it scrolls.
pub const TICKET_TTL: Duration = Duration::from_secs(600);

/// The most tickets held at once, across every reader.
///
/// A ceiling rather than a target. Turning the mode on mints one per visible row, and scrolling
/// mints more; without a cap a page left open all day is an unbounded map. At the cap the
/// least recently claimed go first — never a ticket a read is playing from while an
/// unclaimed one can go (`#2066 vibe-talk-page-freeze`: evicting by mint order killed a
/// live read mid-message once fast held preparations minted bursts of hundreds).
pub const MAX_TICKETS: usize = 2048;

/// How many gone tickets (expired or evicted) keep only their observation, so a failure
/// report arriving after its ticket went is still recorded instead of refused
/// (`#2066`): the correlation key, never the text. Bounded like the store it shadows.
pub const MAX_GONE_TICKETS: usize = 128;

/// One message, resolved and ready to be spoken.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    /// The channel it came from, kept for the log line rather than for the lookup.
    pub channel: String,
    /// The Discord message this speaks. One ticket is one message.
    pub message: String,
    /// The text as it will be SENT: already through `speakable::for_speech`, so the tap does not
    /// pay for normalisation either.
    pub said: String,
    /// The same message block by block, in the order the page draws its blocks, each through the
    /// same speech rules: what `?block=k` says. Empty when the message is one block, which `said`
    /// already is. `#376 read-aloud-blocks`.
    pub blocks: Vec<String>,
    /// The pace the reader had chosen when this was minted, if they had chosen one.
    pub speed: Option<f64>,
    /// The voice the reader had chosen in Settings when this was minted,
    /// for a provider with a catalogue. `#552 wit-tts-provider`.
    pub voice: Option<String>,
    /// Opaque correlation key for content-free server and browser timing records.
    pub observation: String,
    /// Message lookup and normalization, ending before provider connection or readiness begins.
    pub preparation_ms: u64,
}

/// How the audio request itself spent its time, measured on the server.
///
/// The four phases add up to `request_to_first_audio_ms`, so a report can say where the wait went
/// without subtracting numbers taken from different clocks or different requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerTiming {
    /// From the audio request arriving until its first real audio chunk (not a container header).
    pub request_to_first_audio_ms: u64,
    /// Transport establishment this request waited for: zero when it reused a live session, and
    /// includes joining a connection another request (normally preparation) had already begun.
    pub request_connection_ms: u64,
    /// Waiting for a new session to announce itself ready; zero when a live session was reused.
    pub request_session_start_ms: u64,
    /// Waiting for an earlier read on the same session to finish or acknowledge its interrupt.
    pub yield_ms: u64,
    /// The remainder: from the prompt (or, for a provider without sessions, the request) until the
    /// first audio chunk, including any first attempt that failed and was repeated.
    pub first_audio_ms: u64,
    /// Content-free facts about the provider session; `None` for a provider that keeps none.
    pub session: Option<crate::speech::SessionUse>,
}

#[derive(Clone, Debug)]
struct Entry {
    prepared: Prepared,
    minted: Instant,
    /// When this ticket was last claimed (played from), or never yet: a claimed ticket
    /// is a read in progress and is evicted last (`#2066`).
    last_claimed: Option<Instant>,
    /// Per block read (`None` for the whole message), because each is its own request.
    server_timing: HashMap<Option<usize>, ServerTiming>,
    /// `#548 cloud-read-failures`. Why a read's audio stream ended in an error, per block: the error's
    /// code and its reason, for the page to ask after a read that was cut off.
    stream_failures: HashMap<Option<usize>, (String, String)>,
}

/// `#548 cloud-read-failures`. A read-aloud that failed where the reader heard it, as the page reported
/// it: what diagnostics shows of the last one, and how many there have been.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFailure {
    /// The page's short reason: `silent`, `no_audio`, `refused`, and so on.
    pub reason: String,
    /// What the reader was told, bounded.
    pub detail: String,
    /// When it was reported.
    pub at: std::time::SystemTime,
}

/// `#2056 vibe-talk-reader-retry-and-error-telemetry`. Per-day read-aloud
/// error-rate counters: how many reads failed, by class, how many failures were
/// retried, how many retries recovered, how many gave up, and how many provider
/// turns the server interrupted for each of the two reasons it ever does —
/// `abandoned` (the page's audio consumer went away) and `superseded` (a new
/// selection took the floor). Content-free by construction: classes are short
/// slugs the failure routes already validate, and nothing here names a
/// message, a reader, or a provider. Kept for the current UTC day only; a
/// restart begins a new count, as the other health counters do.
#[derive(Debug, Default)]
pub struct ReadAloudCounters {
    inner: Mutex<ReadAloudDay>,
}

#[derive(Debug, Default)]
struct ReadAloudDay {
    /// The UTC day (`YYYY-MM-DD`) the counts belong to; empty until first use.
    day: String,
    /// Failures by class (a page reason or a server stream error code).
    failures: std::collections::BTreeMap<String, u64>,
    /// Failures a retry was attempted for, by class.
    retried: std::collections::BTreeMap<String, u64>,
    /// Retries that went on to play, by class.
    recovered: std::collections::BTreeMap<String, u64>,
    /// Failures given up on after the last try, by class.
    gave_up: std::collections::BTreeMap<String, u64>,
    /// Provider turns interrupted because the reader went away.
    abandoned: u64,
    /// Provider turns interrupted because a new selection took the floor.
    superseded: u64,
}

/// One day's counts, as `healthz` shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadAloudSnapshot {
    /// The UTC day the counts belong to.
    pub day: String,
    /// Failures by class.
    pub failures: std::collections::BTreeMap<String, u64>,
    /// Failures a retry was attempted for, by class.
    pub retried: std::collections::BTreeMap<String, u64>,
    /// Retries that went on to play, by class.
    pub recovered: std::collections::BTreeMap<String, u64>,
    /// Failures given up on after the last try, by class.
    pub gave_up: std::collections::BTreeMap<String, u64>,
    /// Provider turns interrupted because the reader went away.
    pub abandoned: u64,
    /// Provider turns interrupted because a new selection took the floor.
    pub superseded: u64,
}

fn today_utc() -> String {
    jiff::Zoned::now()
        .with_time_zone(jiff::tz::TimeZone::UTC)
        .strftime("%Y-%m-%d")
        .to_string()
}

impl ReadAloudCounters {
    /// An empty set of counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn count(map: &mut std::collections::BTreeMap<String, u64>, class: &str) {
        *map.entry(class.to_owned()).or_insert(0) += 1;
    }

    fn with_day<T>(&self, act: impl FnOnce(&mut ReadAloudDay) -> T) -> Option<T> {
        let mut inner = self.inner.lock().ok()?;
        let today = today_utc();
        if inner.day != today {
            *inner = ReadAloudDay {
                day: today,
                ..ReadAloudDay::default()
            };
        }
        Some(act(&mut inner))
    }

    /// One read-aloud failure of `class` (a page reason or a stream error code).
    pub fn failure(&self, class: &str) {
        let _ = self.with_day(|day| Self::count(&mut day.failures, class));
    }

    /// A failure of `class` was tried again.
    pub fn retried(&self, class: &str) {
        let _ = self.with_day(|day| Self::count(&mut day.retried, class));
    }

    /// A retry of `class` went on to play.
    pub fn recovered(&self, class: &str) {
        let _ = self.with_day(|day| Self::count(&mut day.recovered, class));
    }

    /// A failure of `class` was given up on after the last try.
    pub fn gave_up(&self, class: &str) {
        let _ = self.with_day(|day| Self::count(&mut day.gave_up, class));
    }

    /// A provider turn was interrupted: `abandoned` when the reader went
    /// away, `superseded` when a new selection took the floor.
    pub fn interrupted(&self, abandoned: bool) {
        let _ = self.with_day(|day| {
            if abandoned {
                day.abandoned += 1;
            } else {
                day.superseded += 1;
            }
        });
    }

    /// Today's counts.
    #[must_use]
    pub fn snapshot(&self) -> ReadAloudSnapshot {
        self.with_day(|day| ReadAloudSnapshot {
            day: day.day.clone(),
            failures: day.failures.clone(),
            retried: day.retried.clone(),
            recovered: day.recovered.clone(),
            gave_up: day.gave_up.clone(),
            abandoned: day.abandoned,
            superseded: day.superseded,
        })
        .unwrap_or(ReadAloudSnapshot {
            day: today_utc(),
            failures: std::collections::BTreeMap::new(),
            retried: std::collections::BTreeMap::new(),
            recovered: std::collections::BTreeMap::new(),
            gave_up: std::collections::BTreeMap::new(),
            abandoned: 0,
            superseded: 0,
        })
    }
}

/// A short-lived table of prepared messages, keyed by an unguessable ticket.
#[derive(Debug)]
pub struct SpeechTickets {
    held: Mutex<HashMap<String, Entry>>,
    /// Observations of tickets already gone (expired or evicted), newest last, so a
    /// failure report arriving after its ticket went is still recorded under its
    /// observation instead of refused (`#2066`). Keys and correlation keys only.
    gone: Mutex<std::collections::VecDeque<(String, String, Instant)>>,
    /// The last failed read, and how many have failed since the server started.
    failures: Mutex<(u64, Option<ReadFailure>)>,
}

impl Default for SpeechTickets {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeechTickets {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            held: Mutex::new(HashMap::new()),
            gone: Mutex::new(std::collections::VecDeque::new()),
            failures: Mutex::new((0, None)),
        }
    }

    /// Mint a ticket for one prepared message, and return it.
    ///
    /// Expired entries are dropped on the way past, so a store nobody reads from still shrinks.
    pub fn mint(&self, prepared: Prepared) -> String {
        let ticket = fresh_ticket();
        if let Ok(mut held) = self.held.lock() {
            let now = Instant::now();
            // Gone tickets keep their observation for a late failure report (`#2066`).
            let expired: Vec<(String, String)> = held
                .iter()
                .filter(|(_, entry)| now.duration_since(entry.minted) >= TICKET_TTL)
                .map(|(key, entry)| (key.clone(), entry.prepared.observation.clone()))
                .collect();
            held.retain(|_, entry| now.duration_since(entry.minted) < TICKET_TTL);
            // Only after expiry has been applied, so a store full of dead tickets evicts those
            // rather than a live one somebody is about to tap. Eviction then takes the
            // least recently claimed — unclaimed first, oldest minted among them — so a
            // read in progress keeps its ticket however fast new ones are minted.
            let mut evicted: Vec<(String, String)> = Vec::new();
            while held.len() >= MAX_TICKETS {
                let Some(oldest) = held
                    .iter()
                    .min_by_key(|(_, entry)| {
                        (
                            entry.last_claimed.is_some(),
                            entry.last_claimed.unwrap_or(entry.minted),
                        )
                    })
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                if let Some(entry) = held.remove(&oldest) {
                    evicted.push((oldest, entry.prepared.observation));
                }
            }
            if !evicted.is_empty() {
                tracing::warn!(
                    evicted = evicted.len(),
                    held = held.len(),
                    cap = MAX_TICKETS,
                    "speech ticket store at its cap: evicted the least recently claimed"
                );
            }
            if let Ok(mut gone) = self.gone.lock() {
                for (key, observation) in expired.into_iter().chain(evicted) {
                    gone.push_back((key, observation, now));
                }
                while gone.len() > MAX_GONE_TICKETS {
                    gone.pop_front();
                }
            }
            held.insert(
                ticket.clone(),
                Entry {
                    prepared,
                    minted: now,
                    last_claimed: None,
                    server_timing: HashMap::new(),
                    stream_failures: HashMap::new(),
                },
            );
        }
        ticket
    }

    /// The observation a failure report for `ticket` records under: the live ticket's,
    /// or a gone one's kept for [`MAX_GONE_TICKETS`] departures, or `None` for a ticket
    /// this store never held (or held too long ago). A failure report is diagnostics
    /// the page volunteers after a read died; refusing it because its ticket expired
    /// or was evicted in the meantime loses exactly the reports that matter (`#2066`).
    #[must_use]
    pub fn failure_observation(&self, ticket: &str) -> Option<String> {
        if let Ok(held) = self.held.lock() {
            if let Some(entry) = held.get(ticket) {
                return Some(entry.prepared.observation.clone());
            }
        }
        let gone = self.gone.lock().ok()?;
        gone.iter()
            .rev()
            .find(|(key, _, _)| key == ticket)
            .map(|(_, observation, _)| observation.clone())
    }

    /// What this ticket names, or `None` when it is unknown or has expired.
    ///
    /// The two are deliberately the same answer. Telling a caller which of "never existed" and
    /// "existed and lapsed" applies would let them probe for the difference.
    #[must_use]
    pub fn claim(&self, ticket: &str) -> Option<Prepared> {
        let mut held = self.held.lock().ok()?;
        let entry = held.get_mut(ticket)?;
        if Instant::now().duration_since(entry.minted) >= TICKET_TTL {
            let gone = held.remove(ticket);
            drop(held);
            if let (Some(entry), Ok(mut gone_kept)) = (gone, self.gone.lock()) {
                gone_kept.push_back((
                    ticket.to_owned(),
                    entry.prepared.observation,
                    Instant::now(),
                ));
                while gone_kept.len() > MAX_GONE_TICKETS {
                    gone_kept.pop_front();
                }
            }
            return None;
        }
        // NOT removed on a successful claim. An `<audio>` element may open the URL more than once
        // — a retry, a re-read, a media session restoring itself — and a single-use ticket turns
        // the second of those into a failure the reader cannot explain or act on. The claim
        // itself is noted: it is what keeps a playing read's ticket from eviction (`#2066`).
        entry.last_claimed = Some(Instant::now());
        Some(entry.prepared.clone())
    }

    /// Whether this ticket is live: known and unexpired. The same test as [`Self::claim`], without
    /// the copy and without dropping an expired entry, for checking the credential before the rest
    /// of a request is read.
    #[must_use]
    pub fn knows(&self, ticket: &str) -> bool {
        self.held.lock().is_ok_and(|held| {
            held.get(ticket)
                .is_some_and(|entry| Instant::now().duration_since(entry.minted) < TICKET_TTL)
        })
    }

    /// Attach the provider phases measured by the streaming handler to this observation: of the
    /// whole message (`block` is `None`) or of one of its blocks.
    pub fn record_server_timing(&self, ticket: &str, block: Option<usize>, timing: ServerTiming) {
        if let Ok(mut held) = self.held.lock() {
            if let Some(entry) = held.get_mut(ticket) {
                entry.server_timing.insert(block, timing);
            }
        }
    }

    /// Resolve a browser report to its content-free observation and the server phases of the
    /// same read: the whole message, or the block it names.
    #[must_use]
    pub fn observed_timing(
        &self,
        ticket: &str,
        block: Option<usize>,
    ) -> Option<(Prepared, ServerTiming)> {
        let held = self.held.lock().ok()?;
        let entry = held.get(ticket)?;
        if Instant::now().duration_since(entry.minted) >= TICKET_TTL {
            return None;
        }
        Some((entry.prepared.clone(), *entry.server_timing.get(&block)?))
    }

    /// `#548 cloud-read-failures`. Keep why the audio stream of this read (of `block`, or the whole
    /// message) ended in an error: the voice service's code and reason.
    pub fn record_stream_failure(
        &self,
        ticket: &str,
        block: Option<usize>,
        code: &str,
        detail: &str,
    ) {
        if let Ok(mut held) = self.held.lock() {
            if let Some(entry) = held.get_mut(ticket) {
                entry
                    .stream_failures
                    .insert(block, (code.to_owned(), detail.to_owned()));
            }
        }
    }

    /// Forget why an earlier stream of this read failed: a new one has started, and only its own
    /// failure may answer for it.
    pub fn clear_stream_failure(&self, ticket: &str, block: Option<usize>) {
        if let Ok(mut held) = self.held.lock() {
            if let Some(entry) = held.get_mut(ticket) {
                entry.stream_failures.remove(&block);
            }
        }
    }

    /// Why the audio stream of this read ended in an error, if it did and the ticket is live.
    #[must_use]
    pub fn stream_failure(&self, ticket: &str, block: Option<usize>) -> Option<(String, String)> {
        let held = self.held.lock().ok()?;
        let entry = held.get(ticket)?;
        if Instant::now().duration_since(entry.minted) >= TICKET_TTL {
            return None;
        }
        entry.stream_failures.get(&block).cloned()
    }

    /// `#548 cloud-read-failures`. Keep `failure` as the last failed read.
    pub fn record_failure(&self, failure: ReadFailure) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.0 += 1;
            failures.1 = Some(failure);
        }
    }

    /// How many reads have failed since the server started, and the last of them.
    #[must_use]
    pub fn failures(&self) -> (u64, Option<ReadFailure>) {
        self.failures
            .lock()
            .map(|failures| failures.clone())
            .unwrap_or((0, None))
    }

    /// How many tickets are held. For tests and the diagnostics line; not a stable API.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.lock().map(|held| held.len()).unwrap_or(0)
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 256 bits from the system CSPRNG, URL-safe and unpadded.
///
/// `getrandom` rather than a hash of a counter and a clock: this string is the only thing standing
/// between a stranger and one of the owner's messages, and the two properties that matter are that
/// it cannot be guessed and cannot be predicted from another one.
fn fresh_ticket() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    // A failure here means the OS could not produce randomness, which is not a condition this
    // server can paper over with something weaker — so it panics rather than minting a guessable
    // ticket. In practice `getrandom` on Linux does not fail once the pool is initialised.
    getrandom::fill(&mut bytes).expect("the system CSPRNG must be available to mint a ticket");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A correlation id which carries no message, credential, provider, or deployment identity.
#[must_use]
pub fn fresh_observation() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("the system CSPRNG must be available for timing ids");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(message: &str) -> Prepared {
        Prepared {
            channel: "2222222222".to_owned(),
            message: message.to_owned(),
            said: format!("the text of {message}"),
            blocks: Vec::new(),
            speed: None,
            voice: None,
            observation: fresh_observation(),
            preparation_ms: 1,
        }
    }

    #[test]
    fn a_ticket_names_exactly_what_was_minted_under_it() {
        let tickets = SpeechTickets::new();
        let one_prepared = prepared("1000000000000000001");
        let two_prepared = prepared("1000000000000000002");
        let one = tickets.mint(one_prepared.clone());
        let two = tickets.mint(two_prepared.clone());
        assert_ne!(one, two, "two mints produced the same ticket");
        assert_eq!(tickets.claim(&one), Some(one_prepared));
        assert_eq!(tickets.claim(&two), Some(two_prepared));
    }

    #[test]
    fn an_unknown_ticket_is_simply_unknown() {
        let tickets = SpeechTickets::new();
        assert_eq!(tickets.claim("not-a-ticket"), None);
    }

    #[test]
    fn a_ticket_survives_being_claimed_twice() {
        // An `<audio>` element may open the URL again — a retry, a re-read, a media session
        // restoring itself. Consuming the ticket on first claim turns that into a failure the
        // reader cannot explain, for a message they have already been allowed to hear.
        let tickets = SpeechTickets::new();
        let ticket = tickets.mint(prepared("1000000000000000003"));
        assert!(tickets.claim(&ticket).is_some());
        assert!(tickets.claim(&ticket).is_some(), "the second play failed");
    }

    #[test]
    fn tickets_are_unguessable_and_url_safe() {
        // Both properties matter and they are separate: the string has to survive being a path
        // segment, and it has to be worth nothing to someone who has seen a different one.
        let tickets = SpeechTickets::new();
        let minted: Vec<String> = (0..64)
            .map(|n| tickets.mint(prepared(&n.to_string())))
            .collect();
        let unique: std::collections::HashSet<&String> = minted.iter().collect();
        assert_eq!(unique.len(), minted.len(), "a ticket repeated");
        for ticket in &minted {
            assert_eq!(ticket.len(), 43, "not 256 bits of ticket: {ticket}");
            assert!(
                ticket
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "a ticket needs escaping to sit in a URL: {ticket}"
            );
        }
    }

    #[test]
    fn read_aloud_counters_count_each_outcome_by_class_for_today() {
        // `#2056`: the error-rate counters healthz shows: failures by class,
        // retries, recoveries and give-ups, and the two interrupt causes.
        let counters = ReadAloudCounters::new();
        counters.failure("lost");
        counters.failure("lost");
        counters.failure("silent");
        counters.retried("lost");
        counters.recovered("lost");
        counters.gave_up("silent");
        counters.interrupted(true);
        counters.interrupted(false);
        counters.interrupted(false);
        let shown = counters.snapshot();
        assert_eq!(shown.failures.get("lost"), Some(&2));
        assert_eq!(shown.failures.get("silent"), Some(&1));
        assert_eq!(shown.retried.get("lost"), Some(&1));
        assert_eq!(shown.recovered.get("lost"), Some(&1));
        assert_eq!(shown.gave_up.get("silent"), Some(&1));
        assert_eq!(shown.abandoned, 1);
        assert_eq!(shown.superseded, 2);
        assert_eq!(shown.day.len(), 10, "not a day: {}", shown.day);
    }

    #[test]
    fn a_claimed_ticket_survives_a_mint_flood_and_a_gone_one_still_answers_for_its_failure() {
        // `#2066 vibe-talk-page-freeze`: evicting by mint order killed a live read
        // mid-message once fast held preparations minted bursts of hundreds. A ticket
        // being played from (claimed) outlasts unclaimed ones however old it is, and
        // an evicted ticket still names its observation for a late failure report.
        let tickets = SpeechTickets::new();
        let playing = tickets.mint(prepared("playing"));
        let queued = tickets.mint(prepared("queued"));
        assert!(tickets.claim(&playing).is_some(), "the play did not claim");
        for n in 0..MAX_TICKETS + 10 {
            let _ = tickets.mint(prepared(&format!("flood-{n}")));
        }
        assert!(
            tickets.claim(&playing).is_some(),
            "a ticket being played from was evicted by newer mints"
        );
        assert_eq!(
            tickets.claim(&queued),
            None,
            "the unclaimed oldest was not the one evicted"
        );
        assert!(
            tickets.failure_observation(&queued).is_some(),
            "an evicted ticket's failure report would be refused"
        );
        assert_eq!(tickets.failure_observation("never-minted"), None);
    }

    #[test]
    fn the_store_is_capped_and_evicts_the_oldest_rather_than_growing() {
        let tickets = SpeechTickets::new();
        let first = tickets.mint(prepared("oldest"));
        for n in 0..MAX_TICKETS {
            let _ = tickets.mint(prepared(&n.to_string()));
        }
        assert!(
            tickets.len() <= MAX_TICKETS,
            "the store grew past its cap: {}",
            tickets.len()
        );
        assert_eq!(
            tickets.claim(&first),
            None,
            "the oldest ticket survived while newer ones were minted over it"
        );
    }
}
