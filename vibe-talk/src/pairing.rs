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
//!   same ([`FAILURE_LIMIT`] per [`FAILURE_WINDOW`]). A live code is still granted to a limited
//!   address, so one client's failures cannot lock everyone behind a shared proxy out of pairing.
//!   Comparison runs over every live hash without an early exit.
//! * **Nothing secret is logged.** Mint, redeem and expiry each log one line naming the code by
//!   the first few hex digits of its hash ([`hash_prefix`]), which identifies it across the three
//!   lines and is useless to anyone holding the log.
//!
//! What it does NOT do: it hands out the one write token the server has, so a redeemed link is as
//! good as the token itself, for as long as the token is. Revoking a paired phone means rotating
//! `VIBE_TALK_WRITE_TOKEN`, as it always did. And the link is only as private as the place it is
//! sent. This server keeps it from its own read-scope callers — [`redact_codes`] takes it out of
//! every answer to the read token ([`crate::http::redact_layer`]) and out of everything handed to a
//! model ([`crate::untrusted::neutralize`]) — but anyone else who can read the chat can use it
//! first. The person is then told the link was already used, and the redemption is logged at WARN
//! with the client's address.
//!
//! **A partly leaked code cannot be finished by guessing.** A live code is granted even to an
//! address over its failure limit (behind a proxy that address is everyone's), so the per-address
//! limit alone would not stop someone holding most of a code from trying the rest. Two things do:
//! [`redact_codes`] takes out ANY run of code characters after `pair=`, so a preview cut inside a
//! code shows none of it; and [`GLOBAL_FAILURE_LIMIT`] failures in one window, from all clients
//! together, burn every live code.

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
/// Failed redemptions a client address may make in one [`FAILURE_WINDOW`] before its further
/// failures are answered 429. A live code is still granted.
pub const FAILURE_LIMIT: u32 = 10;
/// The window [`FAILURE_LIMIT`] counts over.
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// The most client addresses whose failures are counted at once. A new address beyond it is
/// refused as limited (fail closed) until an older window lapses, so the table cannot be grown
/// without bound by a caller with many addresses.
pub const MAX_TRACKED_ADDRESSES: usize = 4096;
/// Failed redemptions in one [`FAILURE_WINDOW`], from every client together, that burn every live
/// code. Far above what a person mistyping produces, far below what finishing even a 2-character
/// remainder of a leaked code by guessing needs (4096 tries).
pub const GLOBAL_FAILURE_LIMIT: u32 = 100;
/// Random bytes in a code: 256 bits, base64url without padding, 43 characters.
const CODE_BYTES: usize = 32;
/// What follows a code's marker once [`redact_codes`] has taken the code out.
pub const REDACTED: &str = "[redacted]";
const MARKER: &[u8] = b"pair=";

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
    /// No live code matched, and this address has failed too often lately.
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
    /// Every client's failures in the current window, for [`GLOBAL_FAILURE_LIMIT`].
    all_failures: Option<Failures>,
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
        let hash = digest(presented);
        // Every live hash is compared, all the way through, whichever matches.
        let mut found = None;
        for (index, live) in table.live.iter().enumerate() {
            if constant_time_eq(&live.hash, &hash) {
                found = Some(index);
            }
        }
        // A live code is granted even to a limited address. Behind a reverse proxy every client
        // shares one address, and a limit that refused the right code would let one person's
        // failures lock everyone out of pairing; a 256-bit code is what stops guessing, and the
        // limit only bounds how fast anyone can be refused.
        if let Some(index) = found {
            let burned = table.live.swap_remove(index);
            // WARN, so a redemption is visible at any log level a deployment runs at: a person told
            // "already used" for a link they never opened can find who did.
            tracing::warn!(code = %hash_prefix(&burned.hash), ?client, "pairing code redeemed");
            return Redeemed::Granted;
        }
        // EVERY miss counts toward the global limit, a limited address's included: a limited address
        // is still granted a live code, so its misses are guesses like any other, and one address
        // must not be able to guess forever where ten could not.
        count_toward_global_limit(&mut table, now);
        table
            .failures
            .retain(|_, seen| now.saturating_duration_since(seen.since) < FAILURE_WINDOW);
        if !table.failures.contains_key(&client) && table.failures.len() >= MAX_TRACKED_ADDRESSES {
            return Redeemed::Limited;
        }
        let limit = self.failure_limit;
        let seen = table.failures.entry(client).or_insert(Failures {
            since: now,
            count: 0,
        });
        if seen.count >= limit {
            // One line when an address first crosses the limit, and none for the rest of its
            // window: the access log already has a line per request.
            if seen.count == limit {
                seen.count = limit.saturating_add(1);
                tracing::warn!(
                    ?client,
                    "pairing redemptions from this client are refused for the rest of the minute: \
                     too many failures"
                );
            }
            return Redeemed::Limited;
        }
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

/// Count one failed redemption, from anyone, and burn every live code at [`GLOBAL_FAILURE_LIMIT`]
/// in one window. The count starts again after a burn.
fn count_toward_global_limit(table: &mut Table, now: Instant) {
    let all = table
        .all_failures
        .take()
        .filter(|all| now.saturating_duration_since(all.since) < FAILURE_WINDOW)
        .unwrap_or(Failures {
            since: now,
            count: 0,
        });
    let all = Failures {
        count: all.count + 1,
        ..all
    };
    if all.count >= GLOBAL_FAILURE_LIMIT {
        let burned = table.live.len();
        table.live.clear();
        tracing::warn!(
            burned,
            "{GLOBAL_FAILURE_LIMIT} failed pairing redemptions within a minute: every live pairing \
             code is burned; mint a new one"
        );
    } else {
        table.all_failures = Some(all);
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

const fn is_code_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// `text` with every run of code characters after `pair=` replaced by [`REDACTED`], however
/// short the run. A pairing link is `…#pair=CODE`, and a code cut short by a preview is still most
/// of a code; text that merely resembles one loses a few characters, which is the right side to
/// err on.
#[must_use]
pub fn redact_codes(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if text[at..].starts_with(MARKER) {
            let start = at + MARKER.len();
            let mut run = text[start..]
                .iter()
                .take_while(|byte| is_code_byte(**byte))
                .count();
            // `pair` is code characters, so a run can swallow the next marker's first four bytes:
            // stop before it, so that marker's code is redacted in its turn.
            // Only at the run's end: `=` is not a code character, so a following marker can only
            // be the run's last four bytes and the `=` after them.
            if text[start + run..].starts_with(b"=") && text[start..start + run].ends_with(b"pair")
            {
                run -= 4;
            }
            if run > 0 {
                out.extend_from_slice(MARKER);
                out.extend_from_slice(REDACTED.as_bytes());
                at = start + run;
                continue;
            }
        }
        out.push(text[at]);
        at += 1;
    }
    out
}

/// [`redact_codes`] over a string. Only ASCII is replaced, so the result is still UTF-8.
#[must_use]
pub fn redact_codes_in(text: &str) -> String {
    if !text.contains("pair=") {
        return text.to_owned();
    }
    String::from_utf8(redact_codes(text.as_bytes())).unwrap_or_default()
}

/// Where a code that the next bytes could continue begins at the end of `text`, or `text.len()`
/// when there is none: a tail that is a prefix of `pair=`, or `pair=` followed only by code
/// characters, however many. A stream must hold such a tail back until it knows where the run ends.
#[must_use]
pub fn unfinished_code_start(text: &[u8]) -> usize {
    // `pair=` and a run of code characters to the very end.
    let run = text
        .iter()
        .rev()
        .take_while(|byte| is_code_byte(**byte))
        .count();
    let before_run = text.len() - run;
    if text[..before_run].ends_with(MARKER) {
        return before_run - MARKER.len();
    }
    // A prefix of the marker at the very end. `p`, `a`, `i` and `r` are code characters, so such a
    // tail sits inside the run just measured; `=` cannot, so a tail ending in it is the whole marker,
    // handled above.
    for start in text.len().saturating_sub(MARKER.len() - 1)..text.len() {
        if MARKER.starts_with(&text[start..]) {
            return start;
        }
    }
    text.len()
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
    fn failures_are_limited_per_address_and_a_live_code_is_still_granted() {
        let now = Instant::now();
        let pairing = Pairing::new();
        let minted = pairing.mint_at(None, now, wall()).expect("mint");
        for _ in 0..FAILURE_LIMIT {
            assert_eq!(pairing.redeem_at("wrong", HERE, now), Redeemed::Refused);
        }
        assert_eq!(pairing.redeem_at("wrong", HERE, now), Redeemed::Limited);
        assert_eq!(pairing.redeem_at("wrong", HERE, now), Redeemed::Limited);
        // Another address is not limited by this one's failures.
        let there = Client::of(Some("198.51.100.9".parse().expect("ip")));
        assert_eq!(pairing.redeem_at("wrong", there, now), Redeemed::Refused);
        // The right code is granted even to the limited address: behind a proxy, that address is
        // everyone's.
        assert_eq!(
            pairing.redeem_at(&minted.code, HERE, now),
            Redeemed::Granted
        );
        // The window lapses.
        assert_eq!(
            pairing.redeem_at("wrong", HERE, now + FAILURE_WINDOW),
            Redeemed::Refused
        );
    }

    #[test]
    fn two_redemptions_racing_for_one_code_get_one_grant() {
        for _ in 0..50 {
            let pairing = std::sync::Arc::new(Pairing::new());
            let code = pairing.mint(None).expect("mint").code;
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let racers: Vec<_> = (0..8)
                .map(|_| {
                    let (pairing, code, barrier) = (pairing.clone(), code.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        pairing.redeem(&code, HERE)
                    })
                })
                .collect();
            let granted = racers
                .into_iter()
                .map(|racer| racer.join().expect("join"))
                .filter(|answer| *answer == Redeemed::Granted)
                .count();
            assert_eq!(granted, 1);
        }
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
    fn a_code_is_redacted_however_short_the_run_after_its_marker() {
        let code = "Pq3x_Zr-7AbCdEfGhIjKlMnOpQrStUvWxYz01234567";
        for (text, want) in [
            (
                format!("open https://h.example/#pair={code} now"),
                "open https://h.example/#pair=[redacted] now".to_owned(),
            ),
            (
                format!("<https://h/%23pair={code}|link>"),
                "<https://h/%23pair=[redacted]|link>".to_owned(),
            ),
            // A preview cut inside the code: 41 characters and an ellipsis.
            (
                format!("https://h/#pair={}…", &code[..41]),
                "https://h/#pair=[redacted]…".to_owned(),
            ),
            ("pair=x".to_owned(), "pair=[redacted]".to_owned()),
            // A run that would swallow the next marker stops before it.
            (
                format!("https://h/#pair=xpair={code}"),
                "https://h/#pair=[redacted]pair=[redacted]".to_owned(),
            ),
            (
                format!("pair=pair={code}"),
                "pair=pair=[redacted]".to_owned(),
            ),
            // A code that happens to contain `pair` is redacted whole.
            (
                "pair=AbpairCd=rest".to_owned(),
                "pair=[redacted]=rest".to_owned(),
            ),
            ("pair= and #pair=".to_owned(), "pair= and #pair=".to_owned()),
            (
                "naïve text, no marker".to_owned(),
                "naïve text, no marker".to_owned(),
            ),
        ] {
            assert_eq!(redact_codes_in(&text), want);
        }
    }

    #[test]
    fn a_tail_that_the_next_bytes_could_turn_into_a_code_is_found() {
        assert_eq!(unfinished_code_start(b"data: done\n\n"), 12);
        assert_eq!(unfinished_code_start(b"see #pai"), 5);
        assert_eq!(unfinished_code_start(b"see #pair="), 5);
        assert_eq!(unfinished_code_start(b"see #pair=Pq3x"), 5);
        let long = format!("see #pair={}", "A".repeat(200));
        assert_eq!(unfinished_code_start(long.as_bytes()), 5);
        assert_eq!(unfinished_code_start(b"see #pair=[redacted]"), 20);
        assert_eq!(unfinished_code_start(b"stop"), 3);
    }

    #[test]
    fn a_hundred_failures_from_everyone_burn_every_live_code() {
        let now = Instant::now();
        let pairing = Pairing::with_failure_limit(u32::MAX);
        let minted = pairing.mint_at(None, now, wall()).expect("mint");
        for n in 0..GLOBAL_FAILURE_LIMIT - 1 {
            let client = Client::of(Some(IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + n))));
            assert_eq!(pairing.redeem_at("guess", client, now), Redeemed::Refused);
        }
        let other = pairing.mint_at(None, now, wall()).expect("mint");
        assert_eq!(pairing.redeem_at("guess", HERE, now), Redeemed::Refused);
        assert_eq!(
            pairing.redeem_at(&minted.code, HERE, now),
            Redeemed::Refused
        );
        assert_eq!(pairing.redeem_at(&other.code, HERE, now), Redeemed::Refused);
        // A new code works; the count started again.
        let fresh = pairing.mint_at(None, now, wall()).expect("mint");
        assert_eq!(pairing.redeem_at(&fresh.code, HERE, now), Redeemed::Granted);
    }

    #[test]
    fn one_limited_address_guessing_burns_every_live_code_too() {
        let now = Instant::now();
        let pairing = Pairing::new();
        let minted = pairing.mint_at(None, now, wall()).expect("mint");
        for _ in 0..200 {
            assert_ne!(pairing.redeem_at("guess", HERE, now), Redeemed::Granted);
        }
        assert_ne!(
            pairing.redeem_at(&minted.code, HERE, now),
            Redeemed::Granted,
            "one address guessed past the global limit and kept the code alive"
        );
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
