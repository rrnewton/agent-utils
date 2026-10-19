//! Pairing links: a short-lived, single-use code that signs a phone in. `#369 pairing-link`.
//!
//! Typing a 24-character token into a phone is the worst part of setting this server up for a
//! person, and pasting the token itself into a chat would leave a permanent credential in the
//! chat's history. So an agent that holds the write token asks this server for a CODE instead
//! (`POST /api/v1/pair`), and hands the person a link carrying it in the fragment
//! (`https://host/#pair=CODE`). Opening the link redeems the code (`POST /api/v1/pair/redeem`)
//! for the write token, which the page then stores exactly as its sign-in form does.
//!
//! What makes that safe enough to hand around a chat:
//!
//! * **Single use and short-lived.** A code is burned by its first redemption and dies at its
//!   expiry, five minutes by default and ten at most. A link read in the chat's history an hour
//!   later is worth nothing.
//! * **Only a hash is kept.** The table holds SHA-256 of each code, in memory, never the code;
//!   nothing about it outlives the process, and a restart burns every live code.
//! * **Guessing is hopeless and is also limited.** A code is 32 bytes from the system CSPRNG
//!   (256 bits), so it cannot be guessed; failed redemptions are capped per client address all the
//!   same ([`FAILURE_LIMIT`] per [`FAILURE_WINDOW`]), and the cap answers before the code is looked
//!   at. Comparison runs over every live hash without an early exit.
//! * **Nothing secret is logged.** Mint, redeem and expiry each log one line naming the code by
//!   the first few hex digits of its hash ([`hash_prefix`]), which identifies it across the three
//!   lines and is useless to anyone holding the log.
//!
//! What it does NOT do: it hands out the one write token the server has, so a redeemed link is as
//! good as the token itself, for as long as the token is. Revoking a paired phone means rotating
//! `VIBE_TALK_WRITE_TOKEN`, as it always did.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;

/// How long a code lives when the request does not say.
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);
/// The longest a code may live. A request asking for more is refused rather than shortened, so
/// the caller is never told a link lasts longer than it does.
pub const MAX_TTL: Duration = Duration::from_secs(600);
/// The most codes that may be live at once. Each is a few dozen bytes, so this bounds memory
/// rather than use: a minting agent that leaks codes is told so, and an expired code frees its slot.
pub const MAX_LIVE: usize = 32;
/// Failed redemptions a client address may make in one [`FAILURE_WINDOW`] before it is answered
/// 429 without its code being looked at.
pub const FAILURE_LIMIT: u32 = 10;
/// The window [`FAILURE_LIMIT`] counts over.
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// The most client addresses whose failures are counted at once. A new address beyond it is
/// refused as limited (fail closed) until an older window lapses, so the table cannot be grown
/// without bound by a caller with many addresses.
pub const MAX_TRACKED_ADDRESSES: usize = 4096;
/// Random bytes in a code: 256 bits, base64url without padding, 43 characters.
const CODE_BYTES: usize = 32;

/// A freshly minted code, the one time the code itself exists on the server.
#[derive(Debug)]
pub struct Minted {
    /// The code. Returned to the minting caller and then dropped; never stored, never logged.
    pub code: String,
    /// When it stops being redeemable.
    pub expires_at: jiff::Timestamp,
}

/// Why a code could not be minted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MintError {
    /// The requested lifetime is zero or longer than [`MAX_TTL`].
    #[error("ttl_seconds must be between 1 and {} seconds", MAX_TTL.as_secs())]
    BadTtl,
    /// [`MAX_LIVE`] codes are already live.
    #[error("{MAX_LIVE} pairing codes are already live; wait for one to expire or be used")]
    TooMany,
}

/// The answer to one redemption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redeemed {
    /// The code was live and unused, and is now burned. The caller may hand out the token.
    Granted,
    /// No live code matched: wrong, expired, already used, or not a code at all. Deliberately one
    /// answer, so a caller cannot tell a used code from one that never existed.
    Refused,
    /// This address has failed too often lately; the code was not looked at.
    Limited,
}

/// A client address, as the failure limit counts it. An IPv6 client is counted by its /64, since
/// one host is routinely handed a whole /64 and could otherwise walk through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Client {
    /// The peer's address, or its IPv6 /64.
    Address(IpAddr),
    /// The connection did not say. Every such request shares ONE count, so a deployment that loses
    /// the peer address is limited harder, never not at all.
    Unknown,
}

impl Client {
    /// Count `address` by the unit the limit applies to.
    #[must_use]
    pub fn of(address: Option<IpAddr>) -> Self {
        match address {
            None => Self::Unknown,
            Some(IpAddr::V4(v4)) => Self::Address(IpAddr::V4(v4)),
            Some(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
                Some(v4) => Self::Address(IpAddr::V4(v4)),
                None => {
                    let segments = v6.segments();
                    Self::Address(IpAddr::V6(std::net::Ipv6Addr::new(
                        segments[0],
                        segments[1],
                        segments[2],
                        segments[3],
                        0,
                        0,
                        0,
                        0,
                    )))
                }
            },
        }
    }
}

struct Live {
    hash: [u8; 32],
    expires: Instant,
}

struct Failures {
    since: Instant,
    count: u32,
}

#[derive(Default)]
struct Table {
    live: Vec<Live>,
    failures: HashMap<Client, Failures>,
}

/// The live codes, as hashes, and the failure counts. One per server, in [`crate::state::AppState`].
pub struct Pairing {
    table: Mutex<Table>,
    failure_limit: u32,
}

impl Default for Pairing {
    fn default() -> Self {
        Self::new()
    }
}

impl Pairing {
    /// An empty table with the shipped failure limit.
    #[must_use]
    pub fn new() -> Self {
        Self::with_failure_limit(FAILURE_LIMIT)
    }

    /// An empty table that allows `limit` failures per address per window. For tests that must
    /// send more refused redemptions than the shipped limit allows; the server uses [`Self::new`].
    #[must_use]
    pub fn with_failure_limit(limit: u32) -> Self {
        Self {
            table: Mutex::new(Table::default()),
            failure_limit: limit,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        // A panic while holding this lock leaves nothing half-written that matters: at worst a
        // code is live or burned a moment early. Recover rather than take the route down.
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Mint a code that lives for `ttl` (default [`DEFAULT_TTL`]).
    ///
    /// # Errors
    ///
    /// [`MintError::BadTtl`] for a zero or over-long lifetime, [`MintError::TooMany`] when
    /// [`MAX_LIVE`] codes are live.
    pub fn mint(&self, ttl: Option<Duration>) -> Result<Minted, MintError> {
        self.mint_at(ttl, Instant::now(), jiff::Timestamp::now())
    }

    /// [`Self::mint`] at a given instant, for tests of the expiry boundary.
    ///
    /// # Errors
    ///
    /// As [`Self::mint`].
    pub fn mint_at(
        &self,
        ttl: Option<Duration>,
        now: Instant,
        wall: jiff::Timestamp,
    ) -> Result<Minted, MintError> {
        let ttl = ttl.unwrap_or(DEFAULT_TTL);
        if ttl.is_zero() || ttl > MAX_TTL {
            return Err(MintError::BadTtl);
        }
        let mut table = self.lock();
        expire(&mut table, now);
        if table.live.len() >= MAX_LIVE {
            return Err(MintError::TooMany);
        }
        let mut bytes = [0_u8; CODE_BYTES];
        // As for a speech ticket: the OS failing to produce randomness is not something to work
        // around with a weaker code.
        getrandom::fill(&mut bytes).expect("the system CSPRNG must be available to mint a code");
        let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let hash = digest(&code);
        table.live.push(Live {
            hash,
            expires: now + ttl,
        });
        let expires_at = wall
            .checked_add(jiff::SignedDuration::try_from(ttl).unwrap_or_default())
            .unwrap_or(wall);
        tracing::info!(
            code = %hash_prefix(&hash),
            ttl_seconds = ttl.as_secs(),
            live = table.live.len(),
            "pairing code minted"
        );
        Ok(Minted { code, expires_at })
    }

    /// Redeem `presented` for `client`. On [`Redeemed::Granted`] the code is already burned.
    pub fn redeem(&self, presented: &str, client: Client) -> Redeemed {
        self.redeem_at(presented, client, Instant::now())
    }

    /// [`Self::redeem`] at a given instant, for tests of the expiry boundary and the limit.
    pub fn redeem_at(&self, presented: &str, client: Client, now: Instant) -> Redeemed {
        let mut table = self.lock();
        expire(&mut table, now);
        // The limit answers first, so a limited caller learns nothing about its code.
        table
            .failures
            .retain(|_, seen| now.saturating_duration_since(seen.since) < FAILURE_WINDOW);
        let failed = table.failures.get(&client).map_or(0, |seen| seen.count);
        if failed >= self.failure_limit {
            tracing::warn!(
                ?client,
                "pairing redeem refused: too many failures from this client"
            );
            return Redeemed::Limited;
        }
        if failed == 0 && table.failures.len() >= MAX_TRACKED_ADDRESSES {
            tracing::warn!(
                ?client,
                "pairing redeem refused: too many clients failing at once"
            );
            return Redeemed::Limited;
        }
        let hash = digest(presented);
        // Every live hash is compared, all the way through, whichever matches.
        let mut found = None;
        for (index, live) in table.live.iter().enumerate() {
            if constant_time_eq(&live.hash, &hash) {
                found = Some(index);
            }
        }
        if let Some(index) = found {
            let burned = table.live.swap_remove(index);
            tracing::info!(code = %hash_prefix(&burned.hash), ?client, "pairing code redeemed");
            return Redeemed::Granted;
        }
        let seen = table.failures.entry(client).or_insert(Failures {
            since: now,
            count: 0,
        });
        seen.count += 1;
        let count = seen.count;
        tracing::info!(
            ?client,
            failures = count,
            "pairing redeem refused: no live code matched"
        );
        Redeemed::Refused
    }

    /// Drop every expired code, logging each. The server calls this on a timer, so an unused code's
    /// expiry is logged near when it happens rather than at the next mint or redemption.
    pub fn sweep(&self) {
        expire(&mut self.lock(), Instant::now());
    }

    /// How many codes are live now. For tests and diagnostics; it says nothing about any code.
    #[must_use]
    pub fn live(&self) -> usize {
        let mut table = self.lock();
        expire(&mut table, Instant::now());
        table.live.len()
    }
}

/// Remove the codes whose expiry is at or before `now`. A code is live strictly BEFORE its expiry.
fn expire(table: &mut Table, now: Instant) {
    table.live.retain(|live| {
        let alive = now < live.expires;
        if !alive {
            tracing::info!(code = %hash_prefix(&live.hash), "pairing code expired unused");
        }
        alive
    });
}

fn digest(code: &str) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, code.as_bytes()).as_ref());
    out
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0_u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The first eight hex digits of a code's hash: enough to match a mint to its redemption or expiry
/// in the log, and nothing anyone could redeem.
#[must_use]
pub fn hash_prefix(hash: &[u8; 32]) -> String {
    hash[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// [`hash_prefix`] of a code, for tests that look for it in the log.
#[must_use]
pub fn code_hash_prefix(code: &str) -> String {
    hash_prefix(&digest(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> jiff::Timestamp {
        jiff::Timestamp::from_second(1_800_000_000).expect("timestamp")
    }

    const HERE: Client = Client::Address(IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 7)));

    #[test]
    fn a_code_is_256_bits_of_url_safe_text() {
        let pairing = Pairing::new();
        let minted = pairing.mint(None).expect("mint");
        assert_eq!(minted.code.len(), 43);
        assert!(minted
            .code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        let other = pairing.mint(None).expect("mint");
        assert_ne!(minted.code, other.code);
    }

    #[test]
    fn a_code_is_live_until_the_instant_it_expires_and_not_at_it() {
        let now = Instant::now();
        let ttl = Duration::from_secs(300);
        let pairing = Pairing::new();
        let early = pairing.mint_at(Some(ttl), now, wall()).expect("mint");
        let late = pairing.mint_at(Some(ttl), now, wall()).expect("mint");
        assert_eq!(
            early.expires_at,
            wall()
                .checked_add(jiff::SignedDuration::from_secs(300))
                .expect("add")
        );
        let just_before = now + ttl - Duration::from_millis(1);
        assert_eq!(
            pairing.redeem_at(&early.code, HERE, just_before),
            Redeemed::Granted
        );
        assert_eq!(
            pairing.redeem_at(&late.code, HERE, now + ttl),
            Redeemed::Refused
        );
    }

    #[test]
    fn the_default_lifetime_is_five_minutes_and_the_longest_is_ten() {
        let now = Instant::now();
        let pairing = Pairing::new();
        let minted = pairing.mint_at(None, now, wall()).expect("mint");
        assert_eq!(
            pairing.redeem_at(&minted.code, HERE, now + DEFAULT_TTL),
            Redeemed::Refused
        );
        assert!(pairing
            .mint_at(Some(Duration::from_secs(600)), now, wall())
            .is_ok());
        assert_eq!(
            pairing
                .mint_at(Some(Duration::from_secs(601)), now, wall())
                .unwrap_err(),
            MintError::BadTtl
        );
        assert_eq!(
            pairing
                .mint_at(Some(Duration::ZERO), now, wall())
                .unwrap_err(),
            MintError::BadTtl
        );
    }

    #[test]
    fn a_code_is_good_once_and_several_may_be_live() {
        let pairing = Pairing::new();
        let first = pairing.mint(None).expect("mint");
        let second = pairing.mint(None).expect("mint");
        assert_eq!(pairing.live(), 2);
        assert_eq!(pairing.redeem(&second.code, HERE), Redeemed::Granted);
        assert_eq!(pairing.redeem(&second.code, HERE), Redeemed::Refused);
        assert_eq!(pairing.redeem(&first.code, HERE), Redeemed::Granted);
        assert_eq!(pairing.live(), 0);
    }

    #[test]
    fn a_wrong_code_is_refused_and_burns_nothing() {
        let pairing = Pairing::new();
        let minted = pairing.mint(None).expect("mint");
        for wrong in ["", "nope", &minted.code[..42], &format!("{}A", minted.code)] {
            assert_eq!(pairing.redeem(wrong, HERE), Redeemed::Refused, "{wrong:?}");
        }
        assert_eq!(pairing.redeem(&minted.code, HERE), Redeemed::Granted);
    }

    #[test]
    fn failures_are_limited_per_address_and_the_limit_answers_before_the_code() {
        let now = Instant::now();
        let pairing = Pairing::new();
        let minted = pairing.mint_at(None, now, wall()).expect("mint");
        for _ in 0..FAILURE_LIMIT {
            assert_eq!(pairing.redeem_at("wrong", HERE, now), Redeemed::Refused);
        }
        // Even the right code is not looked at once the address is limited.
        assert_eq!(
            pairing.redeem_at(&minted.code, HERE, now),
            Redeemed::Limited
        );
        // Another address is not limited by this one's failures.
        let there = Client::of(Some("198.51.100.9".parse().expect("ip")));
        assert_eq!(
            pairing.redeem_at(&minted.code, there, now),
            Redeemed::Granted
        );
        // The window lapses.
        let later = pairing
            .mint_at(None, now + FAILURE_WINDOW, wall())
            .expect("mint");
        assert_eq!(
            pairing.redeem_at(&later.code, HERE, now + FAILURE_WINDOW),
            Redeemed::Granted
        );
    }

    #[test]
    fn an_ipv6_client_is_counted_by_its_slash_64() {
        let a = Client::of(Some("2001:db8:1:2:aaaa::1".parse().expect("ip")));
        let b = Client::of(Some("2001:db8:1:2:bbbb::2".parse().expect("ip")));
        let c = Client::of(Some("2001:db8:1:3::1".parse().expect("ip")));
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mapped = Client::of(Some("::ffff:192.0.2.7".parse().expect("ip")));
        assert_eq!(mapped, HERE);
        assert_eq!(Client::of(None), Client::Unknown);
    }

    #[test]
    fn too_many_live_codes_are_refused() {
        let pairing = Pairing::new();
        for _ in 0..MAX_LIVE {
            pairing.mint(None).expect("mint");
        }
        assert_eq!(pairing.mint(None).unwrap_err(), MintError::TooMany);
    }
}
