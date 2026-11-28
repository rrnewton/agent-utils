---
title: 'slack-timeline-budget-drops-reads: the 90 s timeline budget cancels in-flight Slack reads, throws their page away and keeps the timeout as the next answer'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- slack
created_at: 2026-11-28T00:30:00-05:00
updated_at: 2026-11-28T00:30:00-05:00
---

# Description

The private Slack Web API shim in front of the live vibe-talk server (not part of this repository) logs a `BrokenPipeError` whenever it finishes a call and finds that the server already closed the connection. Its log holds 1,924 such lines from 2026-10-15 to 2026-11-27 (latest 10:08 PM ET on 11-27). They come from 524 abandoned calls, about 3.7 lines each. Read-only investigation: logs and code only, with no calls to the live server or to Slack.

**What the log shows (timings and counts only)**

- By method: 364 `conversations.replies`, 144 `conversations.history`, 16 `auth.test`. Every one failed on the body write, after the headers had gone out.
- Shim-side duration of the abandoned `conversations.replies` calls: p10 1,273 ms, p50 1,571 ms, p90 3,490 ms. Answered calls of that method in the last week: p50 1,436 ms, p90 2,031 ms. So abandoned calls are ordinary-length calls. Their durations sit near 1.5 s only because every shim call does. No 1.5 s timer is involved.
- The server already counts these drops itself. A shim-call timer dropped unfinished records `Outcome::Abandoned` (`vibe-talk/src/upstream_profile.rs:729-741`), and the hourly "upstream timings for the last hour" line shows them (for example `conversations.replies abandoned n=3`). The server-side time-to-drop ranges from 28 ms to 1,948 ms after the call was sent. That spread fits a cancel arriving at an arbitrary point in the call, not a per-call timeout.

**Attribution of the 96 events from 2026-11-15 to 2026-11-27 (current code)**

| Events | Cause | Lost? |
|---|---|---|
| 48 | Server restarts and deploys. After the 5 s graceful deadline (`vibe-talk/src/main.rs:26`, `:1344-1362`) the runtime drops every in-flight read. | The process is exiting. The read runs again after start. Harmless. |
| 24 | **The 90 s timeline budget**, direct signal: the cache-first line "the read behind a page answered from the message cache was refused" 0-1.4 s before the EPIPE (19 events), or a kept refusal answered to the page's next poll (5). | Yes. See below. |
| 15 | The same budget, inferred: no line of its own (the relist logs read failures at debug, `vibe-talk/src/relist.rs:196`, and the refresh retry keeps a refusal without logging), but the covering hourly report shows a `fetch_timeline refused` read of about 90,001 ms beside the `abandoned` count. | Yes. |
| 4 | An inline message lookup (`lookup_messages`, `vibe-talk/src/ops.rs:1409-1431`, reported as `fetch_messages abandoned`) whose HTTP caller went away. Axum drops the handler with its in-flight call. | Its caller is gone anyway. Already counted. |
| 5 | No covering hourly report. | Unknown. |

The 428 earlier events (2026-10-02 to 2026-11-14) predate many read-path changes and were not attributed one by one. The 2026-10-29/30 spike (about 72 events) went with page step-back reads of 90-175 s that ended in 400. The older-walk rewrite (`spawn_older_walk`, `vibe-talk/src/ops.rs:2219-2260`) has replaced that path since.

**The cause: `TIMELINE_TIMEOUT` cancels the read instead of bounding it**

- `HttpSlackClient::timeline` wraps `timeline_inner` in `tokio::time::timeout(TIMELINE_TIMEOUT, ...)`, where `TIMELINE_TIMEOUT` is 90 s (`vibe-talk/src/slack/timeline.rs:68`, `:487-492`). On expiry, the whole future is dropped and the error is `refused("the timeline exceeded its time budget; no partial timeline was returned")`.
- A Flat (All) read issues one history read plus a `conversations.replies` read per moved thread, four at a time (`stream::iter(reads).buffered(4)`, `vibe-talk/src/slack/timeline.rs:862`). A catch-up through `fetch_timeline_since` does this for each page it reads.
- Refresh, relist and live reads wait for room under the per-method ceilings: `conversations.replies` ran at its 20-a-minute ceiling, all of it `refresh`, in the minute of the 11-27 10:08 PM ET event. They also wait out rate limits within `RATE_LIMIT_BUDGET` per call (`vibe-talk/src/slack/client.rs:30`, `:415-545`). So a Flat read of a busy conversation can pass 90 s. Inferred from the profile: answered `fetch_timeline` reads in one recent hour ran at p50 14.8 s, p90 38.9 s, max 80.8 s for about 9 calls each, far more than 9 calls at about 1.5 s, four wide, would take.
- At 90 s the timeout drops `timeline_inner`. The up-to-four thread reads in flight are dropped, reqwest closes their sockets, and the shim's answer about 1-2 s later meets a closed connection.
- Hourly reports from 2026-11-15 on show 66 such refused reads at about 90,001 ms (`fetch_timeline` and `fetch_timeline_since`). Together they had sent 934 upstream calls, 6 to 78 per read.

**What is lost, and why it is not harmless**

1. The in-flight call's answer is lost after its quota was spent.
2. The whole page assembled from every earlier call is discarded. Threads whose replies had already settled are kept (`settle_flat` per thread, `vibe-talk/src/slack/timeline.rs:1176`), so the next pass is cheaper. The page, its notice, its owed threads and its live publications are not kept.
3. **The timeout becomes the page's next answer.** `ChatError::Refused` is documented as "the server refused the operation before contacting the provider" (`vibe-talk/src/chat.rs:130-132`). Here it is used for a read that contacted the provider dozens of times. `stand_in_for` classes `Refused` as refused-class (`vibe-talk/src/ops.rs:3589`). So:
   - the read behind a cache-first answer keeps it as `LandedAnswer::Refused` for `OLDER_KEPT`, 120 s (`vibe-talk/src/ops.rs:3280-3316`, `vibe-talk/src/cache_first.rs:572`);
   - the refresh retry does the same without a log line (`vibe-talk/src/ops.rs:3485-3505`), and is never retried;
   - the page's next poll takes it (`vibe-talk/src/ops.rs:2988`) and gets HTTP 400 `refused` (`vibe-talk/src/http/api.rs:108-110`) instead of the cache's page.

   Since 2026-11-15 the page's polls got 50 such 400 answers with no read behind them (42 Flat newest, 3 Flat after, 5 Thread newest), and the cache-first refusal line appears 47 times. On 11-27 at 10:08 PM ET the EPIPE came 1.0 s after the refusal line, and the page's poll 0.4 s before the EPIPE was already answered 400. What the page shows for that 400 was not checked.

**The shim's side** (finding of the earlier read-only pass, commit f721deaa, confirmed): after the EPIPE, the shim tries a second, `internal_error` answer on the same closed connection. It logs two tracebacks per abandoned call, which is why the line count is about 3.7 times the event count.

**Corrections to that earlier pass** (f721deaa was not landed; this file replaces it):

- The 1.5 s first-paint stand-in wait (`FIRST_PAINT_WAIT`, `vibe-talk/src/ops.rs:1957`) does not close connections. The page's read is `tokio::spawn`ed and awaited through `timeout_at(deadline, &mut running)` (`vibe-talk/src/ops.rs:3062-3086`). Dropping a `JoinHandle` detaches the task; it does not abort it.
- The live tail's channel watcher compares a local conversation list and makes no Slack call (`vibe-talk/src/live_tail_supervisor.rs:664-690`). It is aborted only when the tail stops (`:758-760`).
- Server-side abandoned-call counts already exist (above).

# Design

The liaison's requirements for the fix (relayed by the lead coordinator, 2026-11-28) are R1-R5. The parts below meet them.

**1. A timeout is transient: never `refused`, never kept as the next answer (R1).**

- Give the budget expiry its own transient class, for example `ChatError::Timeout` or a `Transport` naming the budget. `upstream_profile::Outcome` then reports it as `timeout`, not `refused`.
- `stand_in_for` answers from the cache: the last good copy, dated (`as_of`), with `refresh_failed` set ("The chat service did not answer in time.").
- `spawn_behind_cache_first` and `spawn_behind_refresh_retry` treat it as an ordinary failure, retried on the refresh back-off and never kept as `LandedAnswer::Refused`. The next poll either answers the stale copy with its flag or takes the retry's landed page.
- Refused-class stays only for real refusals: an expired cursor, access, a channel gone. This part alone ends the 400s.

**2. No silent loss: what the read finished is kept, the rest is owed (R2).** When the read's budget runs out, the Slack timeline returns the page it has instead of being dropped.

- It returns the main page read so far, the replies of every thread whose read completed, and every thread not read (not started, or stopped) in `owed_threads` with the "some threads left out" notice.
- That page lands as an ordinary partial answer (cache fill, live publications, `LandedAnswer::Page`). The existing owed-thread re-read (`#2278`, `spawn_owed_retry` and `spawn_behind_refresh_retry`) then re-queues the owed threads.
- This works for `flat`, `forward`, the history walks (they stop with `has_more` and `next_before`, as on their page budget) and the thread list, and for each page of a `fetch_timeline_since` catch-up.

**3. Queue time is not a hang: the bound must know about the rate wait (R3).** Do NOT just raise 90 s. Replace the single wall-clock `tokio::time::timeout` around `timeline_inner` with two bounds:

- **Page budget (soft).** Checked before admitting each new call, including before an `admit` ceiling or `Retry-After` wait that would end past it. Past the budget, no new call starts and the threads not yet read are owed (part 2). Calls already in flight run to completion and settle. Waiting for room under the 20-a-minute `conversations.replies` ceiling uses up the page budget, so the page answers sooner with more threads owed. It never cancels a call.
- **Hang bound (hard backstop).** A no-progress timer that restarts whenever a call is admitted or answered. It fires only when nothing moves: a call hung on the wire past the HTTP client's own request timeout, or a wait that never ends. Rate waits are progress the client is tracking, so they never trip it.
  - When it fires, the calls in flight are counted as abandoned (already done by the profile), the read returns its partial page with those threads owed, and it logs (part 4).

Equivalent alternative (R3's second option): move the ceiling and `Retry-After` waits out of the cancellable future, so threads queue for admission outside it and only admitted calls run inside the bound. Either way, a read waiting its turn is never treated as a hang.

**4. One line when a bound acts.** One line at INFO when the page budget cuts a read or the hang bound fires: view, reason, calls spent, threads read, owed and in flight. Counts only, never content. The relist's failure line at `vibe-talk/src/relist.rs:196` moves from debug to info for this class, so no budget expiry is silent.

**5. Telling restarts apart (R5).** EPIPEs at shutdown are benign. The coordinators' watch of the shim log will count EPIPEs within a few seconds of a server restart separately, as benign, so that real abandonments stand out. That watch script belongs to the harness, not this repository; the lead coordinator takes it.

**6. Shim (outside this repository; an operator follow-up).** Treat EPIPE or ECONNRESET on the response write as `client_closed`: one INFO line with method and duration, no second `internal_error` write, no traceback.

**Not changed:** the 1.5 s stand-in wait, the per-method ceilings, and drops at shutdown or of inline handlers whose caller left. Those are by design and already counted as abandoned.

# Acceptance Criteria

- **R4 reproduction test.** A fake Slack Web API (the existing Slack provider test servers) has a rate-limited `conversations.replies` route: a ceiling, or 429 with `Retry-After`, so that thread reads queue. Under paused tokio time, a Flat refresh behind a cache-first answer runs past its page budget mid-batch. Then:
  - the read returns `Ok` with the completed threads' replies and the rest in `owed_threads`;
  - the page's next poll answers 200 with those completed threads, not 400;
  - an owed-thread retry is scheduled;
  - the fake server sees no request whose client closed before its answer.
- **Unit:** `stand_in_for` answers from the cache for the budget error, `upstream_profile::Outcome::of` does not report it as `refused`, and no code path keeps it as `LandedAnswer::Refused`. Covers both `spawn_behind_cache_first` and `spawn_behind_refresh_retry`.
- **Queue time is not a hang:** a Flat read whose calls only wait under the replies ceiling (no hung call) is never cut by the hang bound, however long the wait. A test with the ceiling at a few calls a minute and more threads than that shows it ends by the page budget with threads owed.
- **Hang backstop:** with one fake call that never answers, the read ends at the hang bound and logs the counts line. The hung thread is owed, and the next refresh reads it.
- **Live, after deploy, from logs only (no extra calls):** over 24 h of normal use:
  - no `fetch_timeline` or `fetch_timeline_since` `refused` read of about 90 s in the hourly upstream timings;
  - no page poll answered 400 from a kept time-budget refusal;
  - the shim's EPIPE events outside restarts (counted apart by the harness watch) are no more than the server's hourly `abandoned` counts.

# Notes

- **Method:** each shim EPIPE event (log time PT, converted) was placed against the server log by the shim call's window (end minus duration), with lines in the window counted and compared with the same windows shifted 37 s as a baseline. Restart lines, the cache-first refusal line, access lines and the hourly upstream-timing reports carried the signal. Baseline rates for the refusal line and for restarts were 0.2% and 0.8%, against 5% and 26% at the events.
- **Credit:** the earlier read-only pass by mu-clientperf (unlanded commit f721deaa) found the shim's double write and two tracebacks per abandonment. Its cause (the 1.5 s stand-in wait) and two of its fixes (abandoned counts, poller watcher retry) are corrected above.
- **Priority** (liaison): after the owner-UI landings in flight on 2026-11-28. Part 1 is small and self-contained (an error class plus its match arms) and can land first. Parts 2-3 are the larger change.
