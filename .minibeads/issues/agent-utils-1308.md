---
title: 'doc-fetch-since-prompt: Fetch Since visibly asks since when, then fetches with progress'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
created_at: 2026-11-11T15:28:12+00:00
updated_at: 2026-11-12T12:28:47.781820539+00:00
closed_at: 2026-11-12T12:28:47.781820428+00:00
---

# Description

[watermelon-full-preview] Owner reply 363 (2026-11-11 10:21 AM ET): "Fetch Since seems to do nothing... doesn't ask me since when."

Verified on github/main: in `vibe-talk/web/src/view/documents/index.ts` the "since" menu act only unhides `#docs-since`, a days/hours form at the top of `#docs-list-view` in `voice.html`. The menu closes at the same moment, the form is off-screen whenever the list is scrolled to its newest (bottom) end, and nothing focuses or scrolls to it, so the owner never saw it. The form's Go path (`fetchSince` over every loaded row last mentioned at/after the cutoff) is also strictly serial with no progress count, like the old Fetch visible (#1307).

# Design

- Make Fetch Since a visible prompt that asks "since when" where the owner is looking: a dialog, or an inline prompt anchored near the Fetch menu, with the days/hours choice, a Fetch action, and a way to cancel. It must be visible and focused at both phone (393x852) and desktop (1280x800) sizes, however far the list is scrolled.
- Run the resulting fetch with the same concurrency (about 4) and "Fetching done/total" progress as #1307 doc-fetch-visible-viewport, patching cards as they land instead of a full refresh at the end.
- Keep the cutoff semantics: unfetched rows whose last mention is at or after now minus the chosen days/hours. Say in the prompt or the completion status how many documents the cutoff matched.
- Tests wait on events/conditions, never wall-clock budgets (owner rule 346).

# Acceptance Criteria

- Choosing Fetch Since visibly presents a "since when" prompt (in the viewport, focused) at 393x852 and 1280x800, even with the list scrolled to the bottom; cancelling it fetches nothing.
- Confirming it fetches the unfetched documents mentioned since the cutoff with bounded concurrency and a running done/total count on the Fetch button.
- Documents view/browser tests covering prompt visibility, cancel, cutoff scoping, and progress pass at both sizes; the fix is gated at owner tier and landed on its own.

# Notes

Owner reply 363, fast-track. Filed by mu-docfetch before implementation; the fix lands separately on mu-docfetch/doc-fetch-since-prompt. Sibling fixes: #1306 doc-star-gold, #1307 doc-fetch-visible-viewport, #1309 doc-refresh-progress. This fix and #1307 share `list.ts`/`index.ts`; whoever lands second rebases and re-verifies the combined tree. Do not edit `web/voice.js` (#624 in flight); `voice.html` changes for the prompt are in scope if needed.
