---
title: 'vibe-talk-browser-checks-hot-host: browser checks time out en masse in gates on a busy host'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- testing
created_at: 2026-11-21T14:50:00-05:00
updated_at: 2026-11-21T14:50:00-05:00
---

# Description

On 2026-11-21, with the host at about 70% CPU and three gates running, full and path-selected gates of unrelated changes failed 4 to 14 vibe-talk browser checks each (Playwright click or wait timeouts, no assertion failures), on different checks each run; each passes when rerun alone. The owner's rule is that every gating check must pass on a host above 75% CPU and nothing timing-sensitive gates.

# Design

Find why: concurrent Chromium instances inside one gate's CPU allowance, page builds competing with browser steps, or fixed 30 s waits. Then make the browser checks robust on a loaded host: bound browser-step concurrency inside a gate (a dagrun resource), wait on conditions instead of fixed timeouts, and share one built page per gate. Quarantine any check that still fails under load, with a pointer to this issue.

# Acceptance Criteria

- The vibe-talk browser checks pass in a gate with the host above 75% CPU, repeatedly.
- No browser check relies on a wall-clock budget for its verdict.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
