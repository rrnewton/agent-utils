# vt-voice-stream-leak: who leaks the private voice stack's Oxide streams

Issue: #716 vt-voice-stream-leak. Date: 2026-10-31. Scope: read-only
investigation (repository history, deploy history, hourly Oxide STATUS
series since 2026-09-22, bridge/watchdog logs on the private voice host).
No live session was opened and no service was touched; the minimal
reproduction in section 5 is for the coordinator to run, not run here.

## 1. Verdict

The streams are opened by the bridge, one per voice session it starts,
and the evidence points to the bridge not releasing the Oxide (DGW)
stream when the session ends (mechanism not yet confirmed by a live
reproduction; see sections 7 and 8). That non-release most likely
predates the nightly failures — slow accumulation is visible in
earlier runs — and what changed on 2026-10-21 is the opener rate: the
voice watchdog's health probe began opening a session every 15 minutes.
The watchdog and the bridge are both outside this repository; the one
timed bridge contact in vibe-talk (a capability hello, section 4) is
capped well below the observed rate and cannot be the steady source.
The fix belongs to the bridge (release the stream on session
teardown); a probe-side hardening is proposed as a stopgap aid, and the
daily-restart stopgap should stay until the bridge is fixed.

## 2. Evidence

All counts are Oxide `[STATUS]` fields: `streams`, `sessions`,
`backend_sessions`, `push_subs`, sampled hourly.

| Observation | Value | Provenance |
|---|---|---|
| Long run before the leak | 2026-10-13 to 10-21 (8 days, no restart): streams 0 to ~13, i.e. flat; at 10 h uptime streams = 2 | hourly STATUS series, rows 2026-10-14 to 10-21 |
| Watchdog first installed | 2026-10-21 13:12 host-local; timer period 15 min | watchdog unit/timer file timestamps on the host |
| Oxide+bridge restart | 2026-10-21 20:19 UTC (13:19 host-local), 7 min after the watchdog install | hourly STATUS series (uptime resets to 1m) |
| Growth right after | streams 0, 5, 9, 13, 17, 21, 25 at hourly marks from 20:19 UTC: +5 in the first (partial, startup) hour, then exactly +4/hour; `sessions=0`, `push_subs=1` throughout | hourly STATUS series, 2026-10-21T21:00Z to 10-22T13:00Z |
| Failure threshold | nightly "101 but no session_started in 20 s" at 21-23 h uptime, streams ~150-170, `push_subs` ~50 | watchdog failure log 10-28 to 10-31; STATUS tail 10-30/31 (167/56 at 22h47m) |
| Bridge failure signature | "persistent stream reopen failed", then "failed to build oxide client ... Resource temporarily unavailable (os error 11)" | bridge log excerpts, first seen 2026-10-22T13:27Z at streams ~65 |
| Bridge teardown log | per probe session: session opened, then "client session closed", "OxideClient closed", "OxideRuntimeHostState dropped shutdown_started=false aborted_background_tasks=0" | bridge log excerpts 2026-10-22/23 |

Two quantitative checks:

1. **Cadence.** After a +5 startup hour (restart, first probes, and
   server-start hellos land together), growth is exactly +4/hour,
   matching the watchdog's 15-minute probe cadence (4/hour). Hourly
   samples alone cannot prove uniqueness — two 30-minute openers would
   alias to the same slope — but among the voice timers inventoried on
   the host the watchdog is the only 15-minute session opener: the ops
   check runs every 30 min (and only exists since 2026-10-31) and does
   not open voice sessions, the smoke probe is daily (since
   2026-10-31), and real voice use is bursty, not metronomic.
   `sessions=0` in the hourly rows rules out long-lived sessions; it
   cannot see short probe sessions between samples, which is precisely
   the population in question.
2. **Onset.** The 8-day run ending 2026-10-21 crept from 0 to ~13
   streams — not flat in absolute terms, but two orders of magnitude
   slower than what followed, and consistent with occasional real
   sessions each leaving something behind. The fast regime began on
   2026-10-21 (the 10 h-uptime comparison: 2 on 10-14, 45 on 10-22).
   Two changes are confounded that day: the watchdog was installed at
   13:12 host-local and Oxide+bridge were restarted at 13:19. The
   restart alone cannot explain the new rate unless the binary changed
   at that restart (no deploy record either way was found; the host's
   deploy history covers this repo's deploys, and the bridge is not in
   this repo), whereas the watchdog mechanically supplies 4 new
   sessions/hour from that evening on. The parsimonious reading is:
   the bridge's non-release is old, the metronomic opener is new.
   Earlier spikes (e.g. 74 streams on 2026-10-03 in a short run)
   support that reading: accumulation happens whenever sessions happen,
   and only the watchdog made it continuous.

One residual: after 2026-10-26/27 the growth is ~7.5 streams/hour, more
than the session probe's 4/hour. The watchdog's second version
(2026-10-26) added an hourly audio probe, and the bridge-hello path in
section 4 can add up to 2/hour; together these plausibly account for
the excess, but this split was not measured. It does not change the
verdict, because every candidate opener still terminates at the same
bridge session that fails to release its stream.

## 3. Who opens, who fails to release

**Opener (trigger): the watchdog probe.** The probe (outside this repo,
on the host) opens a websocket to the bridge, waits for
`session_started`, then ends the session by sending a `{"type":"quit"}`
text frame and closing the TCP socket. It never sends a websocket
Close frame and does not wait for a close reply. That is abrupt, but it
is a legal way for a client to disappear: real clients also vanish
without a close handshake (the bridge log itself shows "Connection
reset without closing handshake" for these sessions), so the bridge
must tolerate it.

**Non-releaser (root cause): the bridge.** For each probe session the
bridge builds an Oxide client and opens a DGW persistent stream
(`oxide_transport_dgw::persistent_stream: opening DGW persistent
stream`). On disconnect it logs the session and client as closed, yet
Oxide's `streams` count never goes down. The likely mechanism —
inferred from log correlation, not yet from a controlled reproduction —
is that session background work survives teardown:
`aborted_background_tasks=0` on host-state drop shows background tasks
being dropped without abort, which is consistent with a persistent
stream (and later a push subscription) outliving its session. An
Oxide-side counting or delayed-reclamation fault would present the
same way and is not excluded; section 7's reproduction distinguishes
"stream still exists" from "counter never decremented" only in part,
by watching whether EAGAIN arrives on schedule. As streams accumulate,
DGW connects start failing with EAGAIN ("Resource temporarily
unavailable"), the bridge reports "persistent stream reopen failed"
for about an hour, and then new sessions can no longer be built: the
watchdog's "101 but no session_started". This is bridge/Oxide-side
code, not code in this repository.

## 4. Why vibe-talk is not the opener

- The probe connects to the bridge directly; vibe-talk is not in that
  path at all.
- vibe-talk's own bridge contact is `voice_bridge::hello`
  (introduced 2026-10-15 by 0f55bf18, inside the bisect window)
  ([voice_bridge.rs](/home/newton/work/dev-agent-utils/worktrees/slots/voice-leak/vibe-talk/src/voice_bridge.rs:283)):
  it dials, waits for `session_started`, sends the same `quit` frame,
  and then also sends a proper websocket Close
  (`socket.close(None)`, line 332). It runs once at server start and
  then at most every 30 minutes (`HELLO_TTL`, line 52) when a caller
  finds the kept answer stale. (A failed hello is believed for only 60
  seconds, so a caller asking repeatedly during an outage could dial
  once a minute; but a hello that never reaches `session_started` has
  no session and opens no DGW stream, so that path cannot accumulate
  streams either.) At worst it is a minor contributor to
  the post-10-26 excess in section 2, and it is also a victim of the
  same bridge non-release, not its cause.
- Read-aloud and the conversation relay open bridge sessions only while
  a user is actually in a call; `sessions=0` in every leaked row rules
  them out as the steady source.
- The in-repo live-relay check (`vibe-talk/scripts/live-relay-check.py`,
  added 2026-10-19) is not referenced by any unit or timer on the host
  and runs only when invoked by hand.

## 5. Commit bisect, 2026-10-14 to 10-22

Server-side vibe-talk commits touching the voice/relay/read-aloud area
in the window (full list generated from `git log -- vibe-talk/src`):

| Commit | What it does | Can it leak bridge streams? |
|---|---|---|
| 0f55bf18 (10-15) | resume on the websocket backend; also introduces the bridge capability hello | The hello does open bridge sessions (section 4), but at most 2/hour on success and only when a caller asks; it cannot supply a steady 4/hour, least of all overnight with no callers. A minor contributor at most |
| d2d0e327 (10-15) | let a long spoken answer finish | Playback lifetime inside an open call |
| e428af86 (10-18) | read a message into a call exactly once | Requires a live call |
| 28ae7923, 46b0614c (10-18) | record/resume websocket calls | Bookkeeping for live calls |
| 90e5cc97 line (10-19) | opt-in live relay check + guards | Manual script; not on any timer (section 4) |
| a260d5ce, 5e0f114b (10-19) | read-aloud blocks / preparation | User-driven; no timer |
| be547c1b, b283c76f (10-22) | voice-agent catch-ups | Content of live calls; landed a day after onset |

None of these runs on a 15-minute cadence. The fast regime's onset
(2026-10-21 evening) coincides with the watchdog install (13:12
host-local) and a stack restart 7 minutes later; as section 2 notes,
the two are confounded, and the host's `deploy-history.log` covers
this repo's page/chat deploys only, so it cannot prove the bridge
binary was unchanged at that restart. What it does show is that no
repo deploy that evening introduced a new timed opener. Conclusion:
no repo commit in the window can account for the steady leak rate, so
there is no vibe-talk fix or regression test to write; adding one
would test code that is not at fault.

## 6. Proposed changes

**Bridge (owner of the fix; outside this repo).** On session teardown —
quit frame, close frame, or transport reset alike — abort the session's
background tasks and close its DGW persistent stream and push
subscription, so Oxide `streams`/`push_subs` return to baseline after
each session. The `aborted_background_tasks=0` log line is the place to
look first.

**Watchdog probe (hardening; propose to the host operator, do not apply
from here).** After the `quit` frame, send a websocket Close frame
(opcode 0x8) and briefly wait for the peer's close reply before closing
the socket. This may reduce the leak rate if the bridge keys teardown
off the close handshake, but it is not a substitute for the bridge fix:
abrupt disconnects will still occur in the field.

**Keep the stopgap.** The daily 04:00 restart timer stays until a fixed
bridge has survived several days with flat `streams` at `sessions=0`.

## 7. Minimal reproduction (for the coordinator; not run here)

On the private voice host, with the stack freshly restarted:

1. Record Oxide `[STATUS]` (`streams`, `push_subs`) as the baseline.
2. Run the watchdog's session probe once by hand (one websocket
   session that ends with `quit` + socket close, no Close frame).
3. Record `[STATUS]` again: expect `streams` +1, never returning.
4. Repeat with a variant probe that also sends a Close frame: if
   `streams` still grows, the bridge leaks for every disconnect style
   and the probe hardening in section 6 will not help; if it does not
   grow, the hardening is an effective stopgap until the bridge fix.

Expected result from the evidence: step 3 leaks exactly one stream per
probe, matching the observed +4/hour at the 15-minute cadence.

## 8. What is unverified

- The reproduction in section 7 was not run (live-test coordination
  rule); the attribution rests on cadence, onset timing, and bridge
  logs, which agree with each other.
- The post-10-26 growth above 4/hour is attributed only plausibly
  (hourly audio probe, bridge hellos), not measured per opener.
- Whether the bridge releases streams on a proper Close frame is
  unknown; step 4 of the reproduction decides it.
