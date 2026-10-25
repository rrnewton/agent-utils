//! A recap handed to a `vibe-talk-v1` bridge through the prompt it fetches. `#512
//! voice-recap-private-stack`.
//!
//! A bridge that lists `replay` in its `session_started` takes the recap in the protocol's own
//! frame (`#331 resume-on-websocket`; see [`crate::voice_bridge`]). The deployment-managed bridge
//! this was written for lists nothing, so every new call and every reconnect started fresh. It
//! does, though, fetch `GET /api/v1/voice-agent` with the write token as each session opens,
//! reconnects included, and use `prompt.text` verbatim as the model's instructions. So the recap
//! can ride there, with no change to the bridge:
//!
//! 1. Just before it opens a call's socket, the page arms one: `POST /api/v1/voice-recap` with the
//!    conversation to resume from. This server builds the record with [`crate::replay::build`] —
//!    the same budget, the same fence and the same neutralizing as every other recap — and keeps it
//!    for [`RECAP_TTL`], ONE at a time: arming another replaces it.
//! 2. The next write-scope fetch of the profile takes it ([`VoiceRecaps::serve`]): the record goes
//!    into the prompt, before the capability section, and is spent. Never into the fetch made for
//!    this server's own hello (`crate::voice_bridge::BridgeHello::probing`), which opens a bridge
//!    session too and would otherwise spend the recap on a session nobody talks to; never to a read
//!    token, which may not read a transcript.
//! 3. The page asks what became of it (`GET /api/v1/voice-recap/{id}`) once its session has started,
//!    and settles it with `DELETE`, which answers atomically whether it was served or withdrawn. Only
//!    a served recap is drawn as one: "Caught the agent up with N words"; otherwise the call says it
//!    started fresh, and why.
//!
//! # What "served" proves, and what it does not
//!
//! That the bridge fetched a prompt with the record in it while this recap was armed. Not which of
//! its sessions that fetch was for: one page talks to one bridge at a time, the hello is excluded,
//! and the window is a minute, so the session the page just opened is the only candidate in
//! practice, but nothing in the protocol says so. That is the same strength as a `replay_ack`.
//!
//! # Nothing of it is logged, and nothing outlives its use
//!
//! The log says what happened to a recap by number and counts. The text is held only while armed:
//! served, withdrawn or expired, only the outcome is kept, for the page's question. Forgetting the
//! conversations, or the whole store, drops the slot as well.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

/// How long after one of this server's own bridge sessions has started the profile fetch it is owed
/// may still arrive and be taken for it ([`OwnSessions`]). A bridge that fetches as its session
/// opens has fetched long before; one that fetches beside `session_started` may be slower.
pub const OWN_FETCH_GRACE: Duration = Duration::from_secs(5);

/// The bridge sessions this server opens itself — the hello, read-aloud — each of which the bridge
/// fetches the profile for, as it does for a call. `#512 voice-recap-private-stack`.
///
/// Each one is owed exactly ONE fetch, the first write-scope fetch to arrive while it is opening or
/// within [`OWN_FETCH_GRACE`] after: that fetch is withheld the recap, which stays armed for the
/// call. One fetch per session rather than every fetch in a window, so a call opening beside the
/// hello is still served once the hello's own fetch has come; and a session whose fetch never comes
/// stops owing one once its grace is over.
#[derive(Debug, Default)]
pub struct OwnSessions {
    open: AtomicU64,
    state: Mutex<OwnState>,
}

#[derive(Debug, Default)]
struct OwnState {
    owed: u64,
    last_started: Option<Instant>,
}

impl OwnSessions {
    /// Nothing open, nothing owed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One of this server's sessions is opening: it is owed a fetch, and counted open until the
    /// guard drops — once the session has started, or failed to.
    #[must_use]
    pub fn begin(&self) -> OwnSession<'_> {
        self.open.fetch_add(1, Ordering::AcqRel);
        self.lock().owed += 1;
        OwnSession(self)
    }

    /// Whether one of this server's sessions is opening now.
    #[must_use]
    pub fn opening(&self) -> bool {
        self.open.load(Ordering::Acquire) > 0
    }

    /// A write-scope profile fetch arrived at `now`: whether it is one this server's own session is
    /// owed, which is then spent.
    pub fn take_fetch(&self, now: Instant) -> bool {
        let opening = self.opening();
        let mut state = self.lock();
        let recent = state
            .last_started
            .is_some_and(|at| now.saturating_duration_since(at) < OWN_FETCH_GRACE);
        if !opening && !recent {
            state.owed = 0;
            return false;
        }
        if state.owed == 0 {
            return false;
        }
        state.owed -= 1;
        true
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OwnState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One of this server's own bridge sessions while it opens; see [`OwnSessions::begin`].
#[derive(Debug)]
pub struct OwnSession<'a>(&'a OwnSessions);

impl Drop for OwnSession<'_> {
    fn drop(&mut self) {
        self.0.lock().last_started = Some(Instant::now());
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}

/// How long an armed recap waits for the bridge's fetch. A call's socket opens moments after the
/// page arms, and a bridge fetches the profile as its session opens; a minute is room for a slow
/// network, and short enough that a call that never opened leaves nothing for an unrelated session.
pub const RECAP_TTL: Duration = Duration::from_secs(60);

/// What became of a recap, as `GET` and `DELETE /api/v1/voice-recap/{id}` answer it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecapState {
    /// Armed and waiting for the bridge's fetch.
    Armed,
    /// A profile fetch took it into the prompt.
    Served,
    /// The page withdrew it before any fetch took it.
    Withdrawn,
    /// Nothing took it within [`RECAP_TTL`].
    Expired,
    /// Not this server's current recap: replaced by a newer one, forgotten, or never armed.
    Unknown,
}

impl RecapState {
    /// The word a log line and the wire carry.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Armed => "armed",
            Self::Served => "served",
            Self::Withdrawn => "withdrawn",
            Self::Expired => "expired",
            Self::Unknown => "unknown",
        }
    }
}

/// The record a profile fetch takes into the prompt, with the counts that describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    /// Which recap it was.
    pub id: u64,
    /// The whole payload: [`crate::replay::PREAMBLE`], then the fenced record.
    pub text: String,
    /// Words of the lines it holds.
    pub words: usize,
    /// How many lines it holds.
    pub lines: usize,
    /// How many older lines did not fit.
    pub dropped: usize,
}

/// How many earlier recaps' outcomes are kept for the page's question, after a newer one replaced
/// them: a second page arming must not turn the first page's answer into `unknown`.
const OUTCOMES_KEPT: usize = 8;

#[derive(Debug)]
struct Slot {
    id: u64,
    armed_at: Instant,
    state: RecapState,
    /// The record, while armed; dropped the moment it is served, withdrawn or expires.
    record: Option<Served>,
}

impl Slot {
    /// Expire it at `now` if it outlived [`RECAP_TTL`] unserved, saying so once.
    fn age(&mut self, now: Instant) {
        if self.state == RecapState::Armed
            && now.saturating_duration_since(self.armed_at) >= RECAP_TTL
        {
            self.state = RecapState::Expired;
            self.record = None;
            tracing::info!(id = self.id, "voice_recap event=expired");
        }
    }
}

/// The one recap this server holds for the next session, and what became of the last.
#[derive(Debug, Default)]
pub struct VoiceRecaps {
    slot: Mutex<Held>,
}

#[derive(Debug, Default)]
struct Held {
    last_id: u64,
    current: Option<Slot>,
    /// What became of the recaps before it, newest last: `(id, state)`.
    earlier: std::collections::VecDeque<(u64, RecapState)>,
}

impl VoiceRecaps {
    /// An empty holder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm `replay` for the next profile fetch, replacing any recap held now. Answers its number.
    ///
    /// A replay with nothing included is not armed: an empty preamble would claim a continuity
    /// with no record behind it ([`crate::replay`]). The caller says so instead.
    pub fn arm(&self, replay: &crate::replay::Replay, now: Instant) -> Option<u64> {
        if replay.included == 0 || replay.text.is_empty() {
            return None;
        }
        let mut held = self.lock();
        held.last_id += 1;
        let id = held.last_id;
        if let Some(mut old) = held.current.take() {
            old.age(now);
            if old.state == RecapState::Armed {
                tracing::info!(id = old.id, by = id, "voice_recap event=replaced");
                old.state = RecapState::Withdrawn;
            }
            held.earlier.push_back((old.id, old.state));
            while held.earlier.len() > OUTCOMES_KEPT {
                held.earlier.pop_front();
            }
        }
        held.current = Some(Slot {
            id,
            armed_at: now,
            state: RecapState::Armed,
            record: Some(Served {
                id,
                text: replay.text.clone(),
                words: replay.words,
                lines: replay.included,
                dropped: replay.dropped,
            }),
        });
        tracing::info!(
            id,
            words = replay.words,
            lines = replay.included,
            dropped = replay.dropped,
            "voice_recap event=armed"
        );
        Some(id)
    }

    /// Whether a recap is armed at `now`. The hello is not asked while one is, so it cannot open a
    /// session whose profile fetch would arrive in its place.
    #[must_use]
    pub fn armed(&self, now: Instant) -> bool {
        let mut held = self.lock();
        held.current.as_mut().is_some_and(|slot| {
            slot.age(now);
            slot.state == RecapState::Armed
        })
    }

    /// What became of recap `id`, as of `now`: a replaced one answers what it came to, or
    /// `withdrawn` when the newer one replaced it unserved.
    #[must_use]
    pub fn state(&self, id: u64, now: Instant) -> RecapState {
        let mut held = self.lock();
        if let Some(slot) = held.current.as_mut().filter(|slot| slot.id == id) {
            slot.age(now);
            return slot.state;
        }
        held.earlier
            .iter()
            .find(|(earlier, _)| *earlier == id)
            .map_or(RecapState::Unknown, |(_, state)| *state)
    }

    /// Withdraw recap `id` if no fetch has taken it, and answer what it came to: `Served` when one
    /// already did, which is the page's answer that the session was caught up, `Withdrawn` when this
    /// call took it back, or what it was before (`Expired`, `Unknown`).
    pub fn withdraw(&self, id: u64, now: Instant) -> RecapState {
        let mut held = self.lock();
        let Some(slot) = held.current.as_mut().filter(|slot| slot.id == id) else {
            return held
                .earlier
                .iter()
                .find(|(earlier, _)| *earlier == id)
                .map_or(RecapState::Unknown, |(_, state)| *state);
        };
        slot.age(now);
        if slot.state == RecapState::Armed {
            slot.state = RecapState::Withdrawn;
            slot.record = None;
            tracing::info!(id, "voice_recap event=withdrawn");
        }
        slot.state
    }

    /// Withdraw whatever recap is armed, whoever armed it: a page that starts a call it hands no
    /// recap — after a reload lost the number of one it armed — so that call is not handed it.
    pub fn withdraw_any(&self, now: Instant) {
        let id = self.lock().current.as_ref().map(|slot| slot.id);
        if let Some(id) = id {
            self.withdraw(id, now);
        }
    }

    /// The armed recap, for a profile fetch at `now`, spent by taking it. `None` when none is armed,
    /// and when `withheld` says this fetch must not have it — it is owed to one of this server's own
    /// sessions ([`OwnSessions`]) — which leaves it armed for the session it was meant for.
    pub fn serve(&self, now: Instant, withheld: bool) -> Option<Served> {
        let mut held = self.lock();
        let slot = held.current.as_mut()?;
        slot.age(now);
        if slot.state != RecapState::Armed {
            return None;
        }
        if withheld {
            tracing::info!(
                id = slot.id,
                "voice_recap event=withheld reason=own_session"
            );
            return None;
        }
        slot.state = RecapState::Served;
        let served = slot.record.take()?;
        tracing::info!(
            id = slot.id,
            words = served.words,
            lines = served.lines,
            dropped = served.dropped,
            after_ms = u64::try_from(now.saturating_duration_since(slot.armed_at).as_millis())
                .unwrap_or(u64::MAX),
            "voice_recap event=served"
        );
        Some(served)
    }

    /// Drop whatever is held: the owner forgot the conversations, and a recap is a copy of one.
    pub fn forget(&self) {
        let mut held = self.lock();
        held.current = None;
        held.earlier.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{build, ReplayPolicy};
    use crate::store::{Speaker, Turn};

    fn replay(words: &[&str]) -> crate::replay::Replay {
        let turns: Vec<Turn> = words
            .iter()
            .enumerate()
            .map(|(at, text)| Turn {
                speaker: if at % 2 == 0 {
                    Speaker::You
                } else {
                    Speaker::Agent
                },
                text: (*text).to_owned(),
                at_ms: i64::try_from(at).expect("small"),
                detail: None,
            })
            .collect();
        build(
            &turns,
            &ReplayPolicy {
                max_chars: 6000,
                max_turns: 40,
            },
        )
    }

    #[test]
    fn an_armed_recap_is_served_once_and_then_says_so() {
        let recaps = VoiceRecaps::new();
        let now = Instant::now();
        let id = recaps
            .arm(&replay(&["is the build green", "it is"]), now)
            .expect("armed");
        assert_eq!(recaps.state(id, now), RecapState::Armed);
        assert!(recaps.armed(now));
        let served = recaps.serve(now, false).expect("the first fetch takes it");
        assert_eq!((served.id, served.lines, served.words), (id, 2, 6));
        assert!(served.text.starts_with(crate::replay::PREAMBLE));
        assert_eq!(recaps.serve(now, false), None, "a recap was served twice");
        assert_eq!(recaps.state(id, now), RecapState::Served);
        // Withdrawing a served recap takes nothing back: it answers that it was served.
        assert_eq!(recaps.withdraw(id, now), RecapState::Served);
    }

    #[test]
    fn the_hellos_fetch_is_withheld_and_leaves_the_recap_for_the_call() {
        let recaps = VoiceRecaps::new();
        let now = Instant::now();
        let id = recaps.arm(&replay(&["hello"]), now).expect("armed");
        assert_eq!(recaps.serve(now, true), None);
        assert_eq!(recaps.state(id, now), RecapState::Armed);
        assert!(recaps.serve(now, false).is_some());
    }

    #[test]
    fn an_unserved_recap_expires_and_a_withdrawn_one_is_never_served() {
        let recaps = VoiceRecaps::new();
        let now = Instant::now();
        let id = recaps.arm(&replay(&["hello"]), now).expect("armed");
        let later = now + RECAP_TTL;
        assert_eq!(
            recaps.serve(later, false),
            None,
            "an expired recap was served"
        );
        assert_eq!(recaps.state(id, later), RecapState::Expired);
        assert_eq!(recaps.withdraw(id, later), RecapState::Expired);
        assert!(!recaps.armed(later));

        let id = recaps.arm(&replay(&["hello"]), now).expect("armed");
        assert_eq!(recaps.withdraw(id, now), RecapState::Withdrawn);
        assert_eq!(
            recaps.serve(now, false),
            None,
            "a withdrawn recap was served"
        );
        assert_eq!(recaps.state(id, now), RecapState::Withdrawn);
    }

    #[test]
    fn arming_replaces_the_last_recap_and_nothing_is_armed_from_an_empty_record() {
        let recaps = VoiceRecaps::new();
        let now = Instant::now();
        let first = recaps.arm(&replay(&["alpha zulu"]), now).expect("armed");
        let second = recaps.arm(&replay(&["bravo yankee"]), now).expect("armed");
        assert_ne!(first, second);
        assert_eq!(recaps.state(first, now), RecapState::Withdrawn);
        let served = recaps.serve(now, false).expect("served");
        assert_eq!(served.id, second);
        assert!(served.text.contains("bravo yankee") && !served.text.contains("alpha zulu"));

        assert_eq!(
            recaps.arm(&replay(&[]), now),
            None,
            "an empty record was armed"
        );
        // The served one keeps its answer after a third replaces it, for the page still asking.
        let third = recaps.arm(&replay(&["charlie xray"]), now).expect("armed");
        assert_eq!(recaps.state(second, now), RecapState::Served);
        assert_eq!(recaps.withdraw(second, now), RecapState::Served);
        recaps.withdraw_any(now);
        assert_eq!(recaps.state(third, now), RecapState::Withdrawn);
        recaps.forget();
        assert_eq!(recaps.state(second, now), RecapState::Unknown);
    }

    #[test]
    fn each_own_session_is_owed_one_fetch_and_no_more() {
        let own = OwnSessions::new();
        let now = Instant::now();
        assert!(
            !own.take_fetch(now),
            "a fetch with no own session was withheld"
        );
        {
            let _hello = own.begin();
            assert!(own.opening());
            assert!(
                own.take_fetch(Instant::now()),
                "the hello's fetch was not taken for it"
            );
            assert!(
                !own.take_fetch(Instant::now()),
                "a second fetch beside the hello was withheld"
            );
        }
        // Owed after it started, within the grace; never past it.
        {
            let _read_aloud = own.begin();
        }
        assert!(own.take_fetch(Instant::now()));
        {
            let _late = own.begin();
        }
        assert!(!own.take_fetch(Instant::now() + OWN_FETCH_GRACE));
        assert!(
            !own.take_fetch(Instant::now()),
            "an owed fetch outlived its grace"
        );
    }
}
