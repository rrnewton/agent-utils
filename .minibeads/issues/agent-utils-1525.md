---
title: 'fold-marker-on-heading-line: put the fold marker on the heading line in Discuss summaries'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-14T15:46:00+00:00
updated_at: 2026-11-14T15:46:00+00:00
closed_at: 2026-11-14T15:46:00+00:00
---

# Description

Owner request 054 (11-14 10:42 AM ET), verbatim: "Was there some reason we couldn't put the expand collapse controls in the summary on the same line as the headings??" The owner's phone screenshot of a Discuss summary shows each fold's disclosure triangle on its own line above its heading, wasting a row per section.

Cause, verified in the checkout: `foldSectionsByHeading` (`vibe-talk/web/src/view/dom/collapse.ts`, `#1387 discuss-collapsible-sections`) moves the block `<hN>` inside the `<summary>`; a block heading stacks under the summary's inline marker. No stylesheet rule covered `#discuss-answer details > summary`, so the marker and the heading took two lines.

# Design

CSS only, in `vibe-talk/web/styles/voice/22-discuss.css`, the stylesheet that owns the Discuss answer folds (beside `.discuss-collapse-bar` and the struck/fresh fold rules):

- `#discuss-answer details > summary` is a flex row with the native marker suppressed (`::-webkit-details-marker` plus `list-style: none`): a `▸`/`▾` chevron (`::before`, muted as `.help-entry`'s marker) stands at the start of the heading's line.
- The heading element itself stays inside the summary — the document outline, heading anchors and the `#1497` fold keys keep reading it — as a `flex: 1` item with margins zeroed, so a long heading wraps beside the chevron, never under it.
- The row keeps `min-height: 2.75rem` (44 px), the tap-target height `.settings-fold` and `.help-entry` already use; native `<details>`/`<summary>` keeps Enter/Space toggling and the screen-reader button unchanged. No change to `collapse.ts`, so every fold built from that structure takes the same line.

The Documents reader (`#1237`) was checked: it builds no heading folds from `collapse.ts` today (no `<details>` in the reader, none in `21-documents.css`), so there is no second stacked marker to fix; the fix covers the one structure `collapse.ts` emits, which is the Discuss summary. The Expand all / Collapse all bar was left as it stands: it sits above the first heading by design (met before any heading, as `#fold-help` does), and folding it into the first heading's row would couple a global control to one section's fold state; slimming it is a separate, trivial follow-up if the owner wants it.

# Acceptance Criteria

- At phone (~390 px) and desktop widths, each Discuss fold's chevron and heading stand on one line, the heading starting beside the chevron column.
- The whole heading row is the tap target, at least 44 px tall; tap and Enter/Space still open and close the fold.
- A long heading wraps with its second line aligned beside the chevron, not under it.
- The heading element remains inside the summary; fold persistence (`#1497`) and the rest of the Discuss walk are unchanged.
- Browser checks judge drawn geometry on conditions only, no wall-clock waits (owner rule 346).

# Notes

## Resolution

Landed on rrnewton/agent-utils:main as one owner fast-track landing (GATE_SLOTS=3, owner priority). The flex-row rules above in `22-discuss.css`; no TypeScript change. `vibe-talk/tests/discuss_browser.py` now asserts, at 393x852, 390x844 and 1280x800: the heading inside its summary, marker and heading on one line (tops within 12 px), the heading starting right of the chevron column, a row at least 44 px tall, the closed chevron shown, a long heading wrapping beside the chevron (line lefts equal, right of the summary edge), and tap plus Enter toggling the fold closed again. Verified by the owner gate; the landed SHA is reported in the lane handoff.
