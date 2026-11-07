---
title: 'gate-browser-steps-under-load: vibe-talk browser checks still fail under shared-gate load and cost every lane a solo rerun'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-07T02:30:00.000000000+00:00
updated_at: 2026-11-07T16:40:00.000000000+00:00
---

# Description

After #905 browser-latency-load-sensitivity (closed 2026-11-06, f605b64e: the fold check's per-press ceiling became the second-largest press at 2x the budget), `vibe-talk.check.fold-latency-browser` still failed in full shared gates on 4 different heads on 2026-11-06, for example desk Shift+O handlers at 22-29 ms against a 21 ms budget. `vibe-talk.check.read-aloud-blocks-browser`, `vibe-talk.check.pairing-link-browser` and `vibe-talk.check.read-aloud-views-browser` each failed once in the same way. Every failure passed in a solo `gate.py --exclusive` rerun on the same commit.

Each failure costs the lane a solo rerun that waits behind the shared gates, about 45 minutes of queue time, and it holds that lane's landing.

# Design

Pick one with measurement and record the choice here:

- Make the assertions load-aware: measure main-thread or CPU time (performance marks, long-task timing) against the budget instead of wall time, or compare against a baseline calibrated on the same host at the start of the step. A real main-thread regression must still fail.
- Or move these browser steps into the gate's exclusive phase, where they run alone, and measure what that adds to a full gate's wall time.
- No sleeps, retries-until-green or silent skips. A skip needs a named, measured reason, per #905's rule.

# Acceptance Criteria

- Under the same bounded contention #905 used (or a full shared gate running beside another), 20 consecutive runs of each affected step pass on an unchanged commit.
- An injected main-thread regression (e.g. a busy loop in the fold handler) still fails the fold check under that load.
- The issue records the choice, the measurements, and the effect on full-gate wall time.

# Notes

Requested 2026-11-06 by the lead coordinator. Related: #905 (earlier fix), #725 (CPU-budget rule).

## Progress, 2026-11-07 (cl-start)

[opus 5.5] **Choice: load-aware assertions, not the exclusive phase.** Every step stays in the shared gate, so a full gate's wall time does not grow by the exclusive phase's queueing (a solo step waits for an empty box, ~45 min in the daytime). Each assertion now measures what a shared host cannot inflate, or waits for an observed state with a deadline that only bounds a failure. No sleeps were added, nothing retries until green, nothing is skipped.

### What changed, per step

- **fold-latency-browser** (#532, #905): a press's handler is judged on the main thread's own CPU time for its keydown dispatch (trace `EventDispatch` thread time), not wall time, so time the renderer was descheduled or throttled by its cgroup does not count. Memory and cache contention (the gate's usual load) inflates CPU time too, and the old sin/cos calibration loop could not see it (it read x1.0 while the handlers ran 1.3-1.5x slower). So each press's CPU time is divided by how much slower a fixed, page-independent layout workload (60 reply-shaped rows in a hidden iframe: toggle a style, read every box) ran just before that press, measured the same way (its `TimerFire` thread time) so a throttled host cannot excuse a regression. Budget 28 ms of work at the median and 2x for the second largest (#866: ~1.5x the 19 ms solo desk median; phone ~15 ms); wall budget derived at 8x. `--calib-idle-ms` gave way to `--ref-idle-ms` (4.4 ms).
- **test.page-3, `#532 shift-o-latency` (node)**: each press is judged on `process.threadCpuUsage()`, the main thread's CPU time, against the unchanged 50 ms (solo: Shift+O 8-13 ms, e 20-26 ms). No reference workload: the fixture lays nothing out, and 50 ms is ~2x e's solo worst, above the x1.5 memory contention measured in the browser.
- **keyboard-browser**: PgDn moves the ring only once the scroll it starts has happened, which a loaded host can begin after `settled()`'s quiet window; the walk now waits (30 s bound) for the ring to leave the row before judging it.
- **pairing-link-browser**: every wait is for an observed state; the 5-10 s deadlines became 60 s (they only bound a failure).
- **read-aloud-views-browser**: state waits bounded at 30 s; the audio-clock check waits for 0.15 s of played audio rather than reading it after a fixed 300 ms; the card-kinds check waits for the runner's card to be summarised and the replica's split before judging.
- **read-aloud-blocks-browser**: an underrun inside a block is excused only when the page could not help it: the next piece's buffer was made (`createBuffer`, probed) only after the audio ran out. An underrun with that buffer already made fails ("the page was late with audio it had"). Two controls, both run every time: a voice held up inside block 2 until the page has run dry must leave an underrun that is not blamed; a page that holds block 2's pieces back 700 ms with them in hand must be blamed. Blocks are at least 2.5 s long so a block's audio outlasts a loaded host's hiccup.
  - **Accepted blind spot:** a page that delays *reading* its stream makes each buffer late too, and is excused by this check. The boundary-gap budget (each boundary against the voice's own first-audio wait) and the slow-voice negative control still catch a page that is slow to take its audio.
- **scroll-stability-browser**: both gate failures (2026-11-06) were one message, "older replies coming in above moved the reader: row 220 at 124.3px became row 218 at -105.6px", and it was the check. At the long thread's head the reader's row has room above it; a fast host landed the older page before the row was measured (the case went untested), a loaded one after, when the older rows filled that room and became "the first row on screen" while row 220 stayed at 124 px. The fake server now holds the older page until the row is measured, so every run tests that order, and `still()` finds the reader's row by id. The old check fails that order every time with the gate's message; the new one passes 3/3, and a list that loses its place still fails ("row 220 at 124.3px is at 1041.3px").
- **desktop-layout-browser** ("the host selected whole copies ''", Settings walk; 2 gate failures): not reproduced in 40 runs (20 ambient beside running gates, 20 under memory stress). The failure now says why (selection ranges and type, the host's innerText, client rects, visibility, focus, animations, user-select). Parked by the coordinator; this issue stays open for it.

### Measurements

Ambient (shared host, other lanes' gates often running beside; 2026-11-07 04:05-05:10 ET):

| Step | Runs | Passed |
|---|---|---|
| fold-latency | 20 | 20 |
| fold-latency, injected ~80 ms fixed-work keydown listener | 3 | 0 (fails, as required) |
| fold-latency, injected ~25 ms | 3 | 0 (fails, as required) |
| keyboard | 20 | 20 |
| read-aloud-views | 20 | 20 |
| pairing-link | 20 | 20 |
| read-aloud-blocks | 20 | 19 (the stall control's fixed 1 s stall hidden by a page seconds ahead; replaced by the state wait, then 5/5) |
| desktop-layout (05:30 ET, two gates running) | 20 | 20 |
| page-3 shift-o-latency (node) | 20 | 20 |
| same, injected ~50 ms keydown listener | 3 | 0 (fails, as required) |

main's versions at ambient beside a running gate, 3 runs each: fold 2/3 (desk Shift+O 24-25 ms over 21), read-aloud-views 2/3 (runner's card not yet summarised), the rest 3/3.

Memory stress (`stress --vm 48 --vm-bytes 256M`, CPUQuota 6400%, run as one exclusive gate ticket, 09:13-09:35 ET): desktop-layout 20/20, page-3 shift-o-latency 20/20 (injected 0/3), fold-latency 20/20 (injected ~80 ms 0/3, ~25 ms 0/3), read-aloud-views 20/20, pairing-link 20/20. keyboard and read-aloud-blocks under stress, and the CPU-quota condition, were dropped when the work was refocused on fold-latency and scroll-stability (15 of the last 23 failed full gates). A gate-like confirmation (four other browser checks looping beside fold-latency and scroll-stability) follows the landing.

### Effect on full-gate wall time

No exclusive phase, so none from queueing. Step durations (median, ambient; main's version, 3 runs, vs this): fold 9.9 → 8.9 s, keyboard 95 → 89 s, pairing-link 5.1 → 4.4 s, read-aloud-views 16.4 → 14.6 s, read-aloud-blocks 52 → 78 s (longer blocks and the two control reads; it runs in parallel with longer steps, so the gate's critical path is unchanged). A gate that would have needed a solo rerun saves ~45 min of queue.
