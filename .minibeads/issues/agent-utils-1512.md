---
title: 'page-browser-checks-settle-under-load: browser checks settle by condition not wall-clock so gates pass under load'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- test-robustness
created_at: 2026-11-14T03:52:29.262035782+00:00
updated_at: 2026-11-14T03:52:29.262035782+00:00
---

# Description

[watermelon-full-preview] Two vibe-talk browser checks keep failing in gates under host load and passing solo (owner rule 346: no real-time-sensitive gating checks; robust at >75% CPU on 316 threads).

1. vibe-talk.check.reply-continuity-browser at 1280x800-dark: 'the list did not move up just as far as shows the box' (mount bottom 687.59 vs foot 718.48: a 30.9 px late settle, with one chip/tools row landing after the test's settle). keepBoxInView is unchanged; its only scroll act is scrollKeeping.toPosition. So the test's settle is a wall-clock wait, not a page bug. Seen 2026-11-13 in a composer-box gate.
2. vibe-talk.check.offline-cache-phone-browser and offline-cache-desk-browser: failed as the untouched red in at least 3 owner-UI gates/validates on 2026-11-13, passing solo each time. Failure text to be found in recent gate logs or reproduced under load.

# Design

Replace fixed sleeps and settle-by-time with waits on conditions or events: layout stable for N animation frames, a ResizeObserver or scroll-owner event, the specific chip/tools row present, the service worker's ready/controllerchange, cache entries present. Use generous timeouts that fail with diagnostics, never timing assertions. Grep the other *_browser.py checks for the same settle helper and fix shared helpers once. Change tests and test helpers only, not page code, unless a check reveals a real page bug (report first).

# Acceptance Criteria

- reply-continuity 'low' (and tall) case waits until the chip/tools row is present and layout (mount bottom vs foot, scrollTop) is stable over consecutive animation frames before measuring; no fixed 80ms settle decides the verdict.
- offline-cache checks wait on service-worker ready/controllerchange and cache entries present (and drawn state) with generous diagnostic timeouts; no fixed sleep decides the verdict.
- Other *_browser.py checks sharing the same settle helper are fixed once in the shared helper, not per-file copies.
- Each fixed check passes 10x while the host is loaded by a contained CPU burner, started only after host-quiet returns 0 and no hold is active; pass counts reported.
- Local validate and GATE_SLOTS=3 gate pass; an untouched red gets one solo pass, then landing is allowed.

# Notes

Owner rule 346 (no real-time-sensitive gating checks; robust at >75% CPU on 316 threads). Related: #1118 browser-fixed-frame-settle, #1250 vibe-talk-timing-robust.

## Resolution

[watermelon-full-preview] Fixed in tests and test helpers only; no page code changed.

- `vibe-talk/tests/reply_continuity_browser.py`: `Walk.settled()` (the one settle helper every case in the walk shares) now waits until a layout snapshot — list scroll, reply mount, chip/tools row, status line, composer, row count — is identical over consecutive frame-separated samples, within a 30 s failure bound that reports the last samples, instead of two frames plus a fixed 80 ms. Snapshot parts are read in separate evaluates: one combined rect read forced a layout pass that scroll-anchored the list and dropped a live drag selection (probed: each part alone and a plain 600 ms wait left the selection intact; the combined read cleared it). `posted_since` returns once the API record is stable over consecutive polls; the resize (120 ms) and drafts (200 ms) fixed dwells are gone; `wait_for` defaults to the 30 s bound.
- `vibe-talk/tests/offline_cache_browser.py`: `wait_rows`/`wait_pill` and the walks' `until`/`wait_for` condition waits use the 30 s `STATE_S` bound (was 5-10 s); `place()` and `geometry()` judge only measures unchanged over consecutive polls; the Cache Storage / service-worker step waits for settled empty registry state over consecutive polls; the read-scope step judges once the refusal/console record is stable; pull-refresh request counting waits for a stable request record instead of fixed 500/400 ms; the emptied-list `settled()` is layout stability over frame-separated samples. The "40px short with the jump showing" state is established as drawn state in a bounded re-park loop: under the burner the page twice in 20 runs sampled a parking scroll while the composer room was still settling and cleared the scrollback (drawn gap exactly 40 with the jump gone; once followed to the foot at gap 0).
- Other `*_browser.py` checks define their own per-file `settled()` copies (no shared module exists to fix once); they were grepped and are untouched here.

Proof (contained burner `cpuset-alloc run --cores 48`, started only after `host-quiet` returned 0, stopped after): reply-continuity 10/10, offline-cache phone 10/10, offline-cache desktop 10/10 on the final code (desktop rounds on intermediate parking fixes were 9/10 and 9/10 at the jump step above; round 3 on the final loop is 10/10). Solo passes for all three before the proof. The original offline gate failure text on 2026-11-13 was not in this host's gate history; the desk jump failure above is the offline failure reproduced under load instead.
