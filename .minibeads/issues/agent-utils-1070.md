---
title: 'cache-account-mismatch-guard: a mismatched provider account or route snapshot must not erase the message cache'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-08T15:41:55.055152415+00:00
updated_at: 2026-11-08T15:42:08.355726530+00:00
---

# Description

Found while measuring #1058 pr-name-enrichment on 2026-11-08. The render-5 release binary was started with --fake-discord against a COPY of the devbig014 Slack message cache (5,366 messages, 20.7 MB). The fake router has one provider key (fake), so the route snapshot named ChanKeys under fake where the file holds slack. Sink install/arm called MessageCache::retain_only, which sweeps every channel and provider not in the snapshot out of the file (vibe-talk/src/cache/sink.rs Sink::install, Sink::arm, Sink::set_routes; vibe-talk/src/cache/mod.rs MessageCache::retain_only). The copy went to 0 messages and 127 KB. Only the copy was harmed; the real store is intact (verified). A second path has the same shape: the account check in cache/sink.rs (fn check) reads the account the file records for a provider and, on mismatch, erases that provider from the file and records the new account, with one WARN line. On the real store, a config slip of exactly these kinds - a renamed provider key, a valid token for a different workspace or account, a test or fake configuration pointed at the real storage.path - silently wipes the owner message cache at startup. With backfill off (the devbig014 deployment), that history is gone until reads happen to refill it.

# Design

Separate refusing from deleting. When the configured world does not match the file world, the safe default is to serve and fill nothing for the mismatched provider, report a loud Trouble in diagnostics, and keep every row until an owner-explicit action (the existing erase switch, or a recorded account change) says otherwise. Two trigger points to cover:
- Account check (cache/sink.rs fn check): stored account present and different from the client account. Today: erase_provider plus set_account. Wanted: quarantine the provider (Unverified/Suspended-like standing with its own reason), no erase, no set_account over the recorded one.
- Route snapshot sweep (MessageCache::retain_only via Sink::install/arm/set_routes): a snapshot whose provider keys or ChanKeys would sweep stored channels of a provider the file has an account record for, but the snapshot does not name that provider at all. Wanted: retain those rows (still unserved and unfilled while unrouted, exactly as today) instead of erasing them. Ordinary channel removal under the same provider key keeps today behavior.
Deliberately decide the revoked-credential path (credential_refused in sink.rs, Standing::Suspended, erase after revoke or grace): it is privacy-motivated and covered by a_revoked_credential_erases_at_once_and_a_refusal_lasting_a_day_erases, so keep it unless the owner says otherwise, and say which choice was made in the closing commit. The guard above is about a credential that WORKS but is for another account, and about key/snapshot mismatches where no credential was refused at all. Diagnostics (CacheReport) should name the quarantined provider and why, with no channel or message ids, as it does today.

# Acceptance Criteria

- A test starts verification with a working credential for a different account than the file records: every cached row of that provider survives, nothing is served or filled for it, and diagnostics report the quarantine reason.
- A test arms a route snapshot missing a provider key the file holds channels and an account record for (renamed key shape): those rows survive, stay unserved, and return to service when a later snapshot names the key again.
- Ordinary removal (a channel leaving the list under the same provider key), the owner erase switch, and the off switch still erase exactly as today, with their existing tests green.
- The revoked-credential behaviour is either unchanged with its test green, or changed with that test updated and the reason stated in the commit.
- Reproduced first: each new test fails on the code before the change (rows erased) and passes after.

# Notes

Evidence and provenance: #1058 pr-name-enrichment reindex measurement, 2026-11-08, mu-prnames reporting to coordb. The destructive run was against a reflink copy in /tmp; the deployed store at ~/.local/share/vibe-talk was never opened by the measurement binary and its file is unchanged (20,672,512 bytes before and after). Log proof in the measurement run: cache opened at the copy path, provider identified as the fake discord account, and no erase warning was printed before cache-stats read 0 messages - the retain sweep erases without a log line of its own, which is part of the silence this issue asks to fix. The account-mismatch erase in fn check does WARN (the message cache holds messages read as another account of this provider; erasing them). Searched before filing: no open duplicate - nearest are closed #297 cache-coverage-races and #364 message-cache-test-flakes (both name account-check tests), and open #208 persistent-cache, #356 provider-namespace-per-key, #521 server-side-search, none of which cover mismatch-triggered erasure. Filed only, deliberately not fixed here; coordb assigns.
