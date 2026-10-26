//! Single-use tickets that open the voice-socket relay for a browser page.
//!
//! `#553 voice-ws-relay-auth`. A page opens its voice socket with `new WebSocket(url)`, which cannot
//! carry an `Authorization` header, so the bearer token the rest of `/api/` requires has nowhere to
//! go. The page's voice-session call IS bearer-authenticated (write scope, see
//! [`crate::http::api::voice_session`]), so that call mints a ticket and hands it back inside the
//! socket URL, and the relay route admits the socket on the ticket instead of a header.
//!
//! A ticket is the bearer token's stand-in for ONE socket, so it is as narrow as that allows: 256
//! bits from the system CSPRNG (the recipe [`crate::speech_tickets`] uses), redeemed at most
//! once, good for [`TICKET_TTL`] from minting, and held in a capped store. Unlike a speech ticket,
//! which an `<audio>` element may legitimately open twice, a voice socket is opened once, so a
//! second use of the same ticket is refused: a URL copied out of a log or a history entry opens
//! nothing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a minted ticket may wait to be redeemed. The page opens the socket straight after the
/// voice-session call returns, so a minute is generous; a ticket found later is worthless.
pub const TICKET_TTL: Duration = Duration::from_secs(60);

/// The most unredeemed tickets held at once. One per call start; a page that minted and never
/// opened leaves one behind until it expires, so this bounds a burst, not a steady state.
pub const MAX_TICKETS: usize = 64;

/// The unredeemed tickets and when each was minted.
#[derive(Debug, Default)]
pub struct VoiceTickets {
    held: Mutex<HashMap<String, Instant>>,
}

impl VoiceTickets {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a ticket and return it. Expired entries are dropped first, then the oldest live one
    /// while the store is full.
    pub fn mint(&self) -> String {
        self.mint_at(Instant::now())
    }

    fn mint_at(&self, now: Instant) -> String {
        let ticket = fresh_ticket();
        if let Ok(mut held) = self.held.lock() {
            held.retain(|_, minted| now.saturating_duration_since(*minted) < TICKET_TTL);
            while held.len() >= MAX_TICKETS {
                let Some(oldest) = held
                    .iter()
                    .min_by_key(|(_, minted)| **minted)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                held.remove(&oldest);
            }
            held.insert(ticket.clone(), now);
        }
        ticket
    }

    /// Redeem a ticket: `true` once for a live ticket, which is removed; `false` for one that is
    /// unknown, already redeemed or expired, the three indistinguishable on purpose.
    #[must_use]
    pub fn redeem(&self, ticket: &str) -> bool {
        self.redeem_at(ticket, Instant::now())
    }

    fn redeem_at(&self, ticket: &str, now: Instant) -> bool {
        let Ok(mut held) = self.held.lock() else {
            return false;
        };
        held.remove(ticket)
            .is_some_and(|minted| now.saturating_duration_since(minted) < TICKET_TTL)
    }

    /// How many unredeemed tickets are held. For tests; not a stable API.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.lock().map(|held| held.len()).unwrap_or(0)
    }

    /// Whether no ticket is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 256 bits from the system CSPRNG, URL-safe and unpadded, as a speech ticket is.
fn fresh_ticket() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    // A failure means the OS could not produce randomness; minting something weaker would hand out
    // a guessable credential, so this panics instead, as the speech tickets do.
    getrandom::fill(&mut bytes).expect("the system CSPRNG must be available to mint a ticket");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_opens_once() {
        let tickets = VoiceTickets::new();
        let ticket = tickets.mint();
        assert_eq!(ticket.len(), 43, "256 bits, base64url without padding");
        assert!(tickets.redeem(&ticket));
        assert!(!tickets.redeem(&ticket), "a second use is refused");
        assert!(tickets.is_empty());
    }

    #[test]
    fn unknown_and_expired_tickets_open_nothing() {
        let tickets = VoiceTickets::new();
        assert!(!tickets.redeem("not-a-ticket"));
        assert!(!tickets.redeem(""));
        let start = Instant::now();
        let ticket = tickets.mint_at(start);
        assert!(!tickets.redeem_at(&ticket, start + TICKET_TTL));
        assert!(
            tickets.is_empty(),
            "an expired ticket is dropped when tried"
        );
    }

    #[test]
    fn the_store_is_capped_and_sheds_the_oldest() {
        let tickets = VoiceTickets::new();
        let start = Instant::now();
        let first = tickets.mint_at(start);
        for i in 1..MAX_TICKETS {
            tickets.mint_at(start + Duration::from_millis(u64::try_from(i).unwrap()));
        }
        assert_eq!(tickets.len(), MAX_TICKETS);
        let newest = tickets.mint_at(start + Duration::from_secs(1));
        assert_eq!(tickets.len(), MAX_TICKETS);
        assert!(!tickets.redeem_at(&first, start + Duration::from_secs(1)));
        assert!(tickets.redeem_at(&newest, start + Duration::from_secs(1)));
    }

    #[test]
    fn minting_drops_expired_tickets_first() {
        let tickets = VoiceTickets::new();
        let start = Instant::now();
        for _ in 0..10 {
            tickets.mint_at(start);
        }
        tickets.mint_at(start + TICKET_TTL);
        assert_eq!(tickets.len(), 1);
    }
}
