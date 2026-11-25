---
title: 'vibe-talk-search-read-browser-condition-waits'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-24T06:00:00-05:00
updated_at: 2026-11-24T06:00:00-05:00
---

# Description

The search and read browser checks (search-unmatched-browser, read-pause-reply-browser, search-typing-fast-browser) fail under load on waits and settled-window assertions: a debounced server ask firing inside a slow keystroke window, a read never reaching its second block, an opened run not showing its messages before the wait expired. Same shape as #2171: waits that should be conditions are durations.

Harden each wait to a condition (observable page/server state) where one exists, keeping only generous backstops, so the checks pass on a hot host and still fail when the behaviour is wrong.

# Acceptance Criteria

- Each named wait is condition-based (or justified as a pure backstop); the three checks pass under load in a gate and their #2175 known-red entries can drop.

# Notes

## Resolution

Landed as fb4553c9 on github/main (gate pass on 6c4f813b). The search walks' authoritative waits now use the hot-host hang backstop instead of 10-20s wall budgets, and the typing burst runs on the page clock (80ms per key) so a starved host cannot make the settled ask fire mid-burst. read-pause-reply was already condition-based; no change there. The family's interim known-red entries stay until regular gates show the steps green on this fix, then drop per #2175.

Filed by the coordinator from the browser-red plan (mu-clientperf, 2026-11-24); the owning lane refines this file as it works.
