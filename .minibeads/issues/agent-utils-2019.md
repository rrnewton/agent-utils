---
title: 'vibe-talk-server-hits-typing-debounce: the while-typing silence assertion races the search debounce on a hot host'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- search
- browser
created_at: 2026-11-20T18:00:00-05:00
updated_at: 2026-11-20T18:00:00-05:00
---

# Description

[watermelon-full-preview] `vibe-talk/tests/server_hits_browser.py` typed `lantern` with `page.type(..., delay=30)` and asserted the server was never asked while typing (`len(api.searches) == 0`, line 341). On a hot host Playwright's per-keystroke cost exceeds the page's debounce/rest window, so the page asks once typing pauses — exactly as designed — and the assertion fires: gate evidence `searches == [{'q': ['l'], ...}]`, one ask for the first letter after a pause. Red in an agentctl-only gate as well, so flaky on main with no relevant change. Verdict reported before fixing: a wall-clock test problem (owner rule 346), not a page bug; the following assertions (one settled ask standing 50 hits as cards) already prove the intended behaviour.

# Design

- Type with a single fill/input event (`page.fill("#search-field", "lantern")`), never a per-key `delay=` race against the debounce. No debounce test hook is exposed by the page, so no mid-typing silence check is kept.
- Assert by condition and count after the settle: exactly one ask, for the full `lantern`, then the existing 50-card stand. The loaded-card filter check becomes a condition wait (`walk.until` on the shown count) instead of an immediate read.

# Acceptance Criteria

- `server-hits-browser` passes solo and in gate on a loaded host, with no `delay=` typing and no while-typing zero-ask assertion.
- The settled behaviour is still proven: exactly one server ask for the whole word and 50 in-channel hits standing as cards.

# Notes

[watermelon-full-preview] mu-discussfix, after #2014. Number 2019 pre-assigned by coordb. Fix option (1) chosen by the coordinator: fill/single input event, one ask for the full word after a condition-based settle, plus the 50-card stand.
