---
title: 'label-inline-fallback: show header labels inline when the meta line has no room'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- labels
- ui
created_at: 2026-11-18T06:34:26-05:00
updated_at: 2026-11-18T06:34:26-05:00
---

# Description

[watermelon-full-preview] Follow-up from #1825 vibe-talk-hashtag-labels and owner thread 168. Today a message's hashtag labels always render as chips in the card meta line, and a body line containing only those hashtags is hidden whenever the chips stand. There is no placement branch for the owner's "unless there isn't space for it in which case they can stay in line" case: if the meta line cannot hold the labels cleanly, the labels should remain inline in the body instead of forcing header chips.

# Design

- Design approved by coordb as written (2026-11-18): one label row or inline fallback, measured on width changes only, search unchanged, stored text untouched. Do not code until coordb lifts the landing freeze.
- Measure placement from the rendered meta line, not from a viewport-width guess: the header keeps the chips only when they fit within the room the meta line can give them without displacing or wrapping beyond the accepted label row.
- When there is no room, do not render the header chips for that row and do not hide its hashtag-only body line; the original hashtags stay inline in the rendered body. Stored text, raw view, and copy remain untouched either way.
- Exercise both widths the owner matrix uses: phone 412x915, including a real `visualViewport` shrink for the on-screen keyboard, and desktop 1280x800; include light and dark where placement is theme-independent but screenshot coverage is not.
- This issue lands in its own worktree, separately from #1835 label-dialog-placement.

# Acceptance Criteria

- A fixture row whose labels fit the meta line shows header chips and hides its hashtag-only line, as #1825 does now.
- A fixture row whose labels do not fit shows no header label chips and keeps the hashtag line visible inline in the body; the labels remain discoverable through `label:<slug>` search.
- The decision is per-row and stable across redraws, theme changes, and the phone keyboard opening or closing; it does not oscillate as the list scrolls.
- A row whose labels change after a live edit or update re-measures and can move between the header and inline placements.
- Browser coverage at phone and desktop proves both branches, with a real keyboard-height viewport shrink on the phone; screenshots are inspected and sent to coordb before landing.

# Notes

Deferred explicitly by the #1825 liaison audit (2026-11-18): the landed implementation always places labels in the meta line. Automatic topic labels and Pinned-as-a-built-in-label are separate owner questions and are not part of this issue.

## Resolution

[watermelon-full-preview] Landed on main as 2784a3ea: chips stand only on one visual label row (list-ResizeObserver width-only measurement; height changes never re-measure; live edits re-measure on the next pass), otherwise the hashtag line stands inline. Screenshots in /tmp/mu-inline-shots (gate pass on 7614ddf2; the keyboard-browser gate flake passed its exclusive rerun).
