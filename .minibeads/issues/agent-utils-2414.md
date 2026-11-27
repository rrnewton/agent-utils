---
title: 'search-typing-fast settled-window assertion recurs after #2177: condition-wait hardening for the head-room and moved-frames check'
status: open
priority: 3
issue_type: task
labels:
- vibe-talk
- flake-triage
- validation
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-27T17:46:00-05:00
---

# Description

#2177 (closed, fb4553c9) hardened the search-typing settled window, yet the same signature recurred on 2026-11-27.

- Head dea5111992d4, ticket 1795754766551248145-8b714a4d, the only red log the audit found on disk:
  `AssertionError: light-desk: foot: the reading row moved after the key (39 frames, up to 39 px) or the head room flipped (0 frames) before the answer settled`
- `vibe-talk.check.search-typing-fast-browser` was red on 14 heads in the audit window (FLAKE 11, of which 6 carried the #2175 known-red label).

# Design

- Wait for the settled state as a condition instead of counting frames inside a window, and keep a generous backstop (owner rule 346).
- Keep the #2175 interim known-red entry until this lands.

# Acceptance Criteria

- The step passes at load1 >= 237 for a whole run, repeatedly, with no wall-clock window deciding the verdict.
- The #2175 entry for this step is dropped once this lands.

# Notes

Filed 2026-11-27 at the lead coordinator's request.
Source: the harness repository's `ai_docs/reports/measurements/2026-11-27-frequent-red-steps-audit.md`, item P7 and step 15.
Follow-up to #2177.
