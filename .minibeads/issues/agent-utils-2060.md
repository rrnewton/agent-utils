---
title: 'vibe-talk-realistic-testing: a local clone of real message shape, and a read-only live test client'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- testing
created_at: 2026-11-21T10:44:00-05:00
updated_at: 2026-11-21T11:05:00-05:00
---

# Description

[watermelon-full-preview] Owner requests 285-287 (2026-11-21): flex both
testing muscles -- hermetic Playwright in a box, and driving a client
against the live server to observe the lags the owner sees. The vast
majority of live operations are read-only, so extensive live testing is
authorised under the binding conditions in Part B below.

## Part A -- the reproducer local clone (landed in this issue's first commit)

`vibe-talk/scripts/local_clone.py`: builds/uses a server binary for a
named SHA (deployed build by default), copies the real message cache
with SQLite's backup API from a read-only open into a 0700 directory
(file 0600, local to the host), starts the binary on a free loopback
port with fake credentials and a loopback stub backend (the stub names
only the provider account the cache was read as, so cache-served reads
verify; every other backend call is refused locally), prints the URL,
and deletes the copy on exit. No live token is ever read; no message
content is printed, logged, or committed (the probe reports HTTP
status, byte counts, and ticket counts only).

Reproducer shape (ids only): one channel, one thread of 15 messages;
the root message prepares as 53 spoken blocks. Verified 2026-11-21:
the clone serves that thread from the copied cache, and speech
preparation of every message in it returns 200 on the deployed SHA
(93cf0b15). The #2055 vibe-talk-server-panic-char-boundary panic (7x in
the deployed log, same char-boundary message) fires during live
read-aloud/playback turns interleaved with timeline reads, and did NOT
reproduce through prepare on this thread alone -- the panicking
internal-link message is likely another on-screen message, so
mu-panic (#2055 vibe-talk-server-panic-char-boundary), mu-readretry
(#2056 vibe-talk-reader-retry-and-error-telemetry) and mu-arrows
(#2058) should drive the clone's full page, not only prepare. The
code-level fix fe0c750a carries its own recorded repro (its new test
panics on unfixed code; 2012 lib tests pass with it).

## Part B -- read-only live-server test client (APPROVED 2026-11-21)

Approved in writing by coordb 2026-11-21, with binding amendments
B1-B6 (recorded verbatim in the approval; summarised here):

- B1 Shared voice session: the live read-aloud conversation session is
  shared with the owner, so for the test credential the server must
  REFUSE server-side read-aloud and speech routes on the conversation
  backend (a distinct refusal). Read-aloud latency is measured with
  the page's device voice on the client, or not at all.
- B2 Owner quota: the test credential gets cache-served reads only;
  any read that would hit the chat provider is refused (or served
  cache-only, marked stale). Plus the pacing/budget-watch stop rule.
- B3 Credential lifecycle: issued by coordb as a random token; the
  live config stores only what the auth layer needs (a hash if the
  server supports it); the client token lives in a 0600 file outside
  every git tree. Revocable by config deletion, plus a runtime kill
  switch (a file flag the server checks per request) that disables
  it without a restart.
- B4 Deploy: the server change ships through a gated landing plus a
  deploy; adding the credential to the live config happens only at a
  deploy restart coordb schedules, announced to the owner.
- B5 Proof on the clone BEFORE any live call: every mutating route
  refuses (listed, each tested), mark-read never writes (dismissal
  and own-read counts stay 0 across a full page session),
  conversation speech refuses, cache-only reads hold, and the kill
  switch works. Results recorded in this issue.
- B6 The live run: short (minutes), in a liaison-confirmed quiet
  period, timings and error classes only, report to coordb after.

1. Credential class (server change, gated, landed before any live
   run): a dedicated test credential, issued for testing only, never a
   copy of the owner's token. The server maps it to a read-only scope
   that refuses every mutating route (post, react, archive, Done /
   mark-read, settings writes) with a distinct refusal, enforced in
   the server's authorization layer -- not by client promise.
2. Mark-read off, server-side: for this credential the server never
   writes a dismissal/own-read decision, and client-config reports
   mark-read disabled for the session, so the page cannot mark read
   by itself. Proven on the local clone first: reads succeed, every
   write route refuses, and the clone store's dismissal count stays
   zero across a full session.
3. Pacing: cache-served routes preferred; a fixed low request rate;
   the server's Slack budget (diagnostics) watched throughout; the
   run stops if limited/refused counts rise.
4. Recording: timings and error classes only -- arrow-to-paint,
   scroll, and read-aloud start/stop on the reproducer message.
   Never message content.
5. Client: an Android emulator leased the sanctioned way if coordb
   confirms one is available, otherwise phone-size Chromium with
   network throttling. Running the client from another host (owner
   287) goes through coordb asking that host's coordinator; this lane
   does not reach that host itself.

## Part B -- B5 clone proof (2026-11-21, on the Part B binary)

Run through `scripts/local_clone.py --test-token` against a fresh
release build of the Part B commit. Statuses and counts only:

- client-config for the test credential: 200, `test_read_only: true`.
- Newest timeline page: 200, 50 messages, cache-served; no provider
  refresh is spawned for a test caller.
- Thread-view read (not cache-served): 403 `test_read_cache_only`.
- Mutating routes, each 403 `forbidden`: POST reply, POST dismiss,
  POST read (mark-read), DELETE read, POST upstream-read,
  POST /channels, PUT client-settings, POST own-read/apply.
- Speech prepare: 403 `test_read_speech_refused` (B1). MCP: 403.
- Mark-read never writes: the timeline `dismissed` count (33, after a
  deliberate write-token dismiss) stayed 33 across a test session of
  timeline reads plus a dismiss attempt.
- Kill switch: creating the kill file made the test credential 401
  (the write credential still 200); removing it restored 200 (B3).
- Unit suites: cargo test --lib 2010 passed; auth tests include the
  test-token scope. Three integration failures (browser validators,
  page build/module suites) are the host's node v16 lacking
  `--test-reporter=tap` -- environmental, also present without this
  change's paths; the gate's node22 steps are the arbiter.
- Hashed token storage: the auth layer has no hash support and no
  SHA-2 dependency is in the lockfile, so the token is stored as the
  existing Secret type; B3's hash clause is conditional on support.

## Part B -- cold-review fix (deny-by-default)

The cold review of 19e8be93 found the scope allow-by-default. Fixed:
the test credential is refused at the ReadScope extractor on every
route an explicit allowlist does not name (client-config and the
channel timeline only -- what the clone needs), with a distinct
`test_read_route_refused`; speech routes refuse through a dedicated
extractor before Path/Query/Json (#41/#42). may_serve is narrowed
back to write-only, with timeline serving for the test credential
local to ops::timeline and its refresh claim suppressed; the
TestRead refusal now precedes the live-fed stand-in (whose thread-list
refresh reads upstream). Token collisions with voice_token and
ingest.token are refused at config load; the kill switch fails closed
and can come from the environment; the access log has a test-read
class. Tests (tests/test_read.rs): every non-allowlisted route in an
enumerated table returns 403 with the provider fetch count unchanged,
the allowlisted timeline refuses rather than fetch when unserved,
speech/MCP refuse distinctly, and the read token keeps its routes.
Re-verified on the clone with the fixed binary: same B5 matrix, plus
diagnostics and messages now answer test_read_route_refused.

# Acceptance Criteria

- Part A: the script above, with the clone verification recorded here.
- Part B: coordb's written approval of this design, then the
  credential-class server change proven on the clone, then -- and
  only then -- a paced live run recording timings and error classes.

# Notes

Part A landed first so the three waiting lanes can use the clone.
Part B's server change is a separate, later commit under this issue.
