---
title: 'linked-message-always-expanded: going to a linked message shows it in full'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-15T15:10:00.000000000+00:00
updated_at: 2026-11-15T15:10:00.000000000+00:00
---

# Description

Owner UI request 095 (2026-11-15, fast-tracked), part 2 of 3: "when I click on
a message link in the summary, I want it to go to the linked message as it
does now. But I want to make sure that whenever we go to a linked message ever
that it uncollapses the target message so we see it in full."

Whenever the page navigates to a linked message — summary/Discuss citation
links, in-app message links (`#543 in-app-message-links` and successors),
`:email:` short refs (`#1565 vibe-talk-short-ref-links`), thread-root links,
search results, any "jump to" — the target message must be uncollapsed so it
shows in full. That covers a collapsed long message, a folded quote, a
combined/merged row (`#1557`), and any "show more" truncation. The expand is
applied at the single navigation entry point, not per caller.

Sibling lanes from the same owner message: `#1584 grip to the left`
(mu-gripleft) and `#1586 full-screen Discuss drops to half on a message link`
(mu-discussdrop), each landing on its own.

# Design

Every message landing already funnels through one reveal: `landOnDeepLink`
in `web/src/view/shell/navigation.ts` (message links, deep links, short refs,
Discuss citations via `onJump`, sent-message jumps) and the reply landings
(`landOnAnswered`, `landOnReply`) all end in `revealRow` in
`web/src/view/channel/replyarrows.ts`, which scrolls the row to the head of
the list and lights it. The bookmark jump (`landOnBookmark`) is the one other
"jump to a message" and gets the same treatment.

A new helper `expandLandedRow` in `web/src/view/channel/fold.ts` opens a
landed row whole: a folded row is opened by clicking its own fold control,
and any fold drawn inside the row (`<details>`: a folded quote, a "show
more") is opened with it. Using the row's own control matters: the fold's
act updates the kept-open record (`expandedIds` in
`web/src/view/channel/folding.ts`), so the opening survives redraws exactly
as if the reader had tapped More themselves.

`revealRow` calls it before measuring the row, so the landing scroll and the
highlight use the full height. `landOnBookmark` calls it the same way. A
combined row has one fold for all its constituents, so a jump to any
constituent opens the whole row. A target with no fold, or already open, is
left exactly as it stands.

Collapse state is per-view state (kept on the device, not on the server):
a landed message stays expanded until the user collapses it again — the
landing only ever opens, never closes.

No touch to `vibe-talk/web/voice.js` or `web/src/app/main.js`.

# Acceptance Criteria

- One browser test per entry point, at phone (~390 px) and desktop widths,
  each showing a collapsed target ends up expanded and visible:
  `vibe-talk/tests/linked_expand_browser.py` covers an in-app message link,
  a reply's arrow, a combined row's second constituent, an `:email:` short
  ref, and the address the page opens by (deep link, by reload standing on
  the address a jump wrote), in real Chromium at 390x844 and 1280x800.
  Discuss citations route through the same `landOnDeepLink` landing and are
  covered by that path; no separate Discuss fixture is added.
- Unit tests where logic changed: `expandLandedRow` opens a folded row by
  its own control, opens folds inside it, and touches neither an open row
  nor a row with no fold (`tests/js/view/channel.test.mjs`).
- Screenshots at 390 px and desktop of the landed, fully shown message,
  looked at by the author.
- `make typecheck` and the page suites pass; the change gates and lands
  under the repository's direct-to-main rules.

# Notes

Files: `vibe-talk/web/src/view/channel/fold.ts` (the helper),
`vibe-talk/web/src/view/channel/replyarrows.ts` (every landing's reveal),
`vibe-talk/web/src/view/channel/bookmark.ts` (the bookmark jump),
`vibe-talk/web/src/view/channel/index.ts` (the export),
`vibe-talk/tests/js/view/channel.test.mjs`,
`vibe-talk/tests/linked_expand_browser.py`, `vibe-talk/tests.dag.yaml`.

Deconflicted with the sibling lanes before editing: mu-gripleft owns
`web/voice.html`, `web/styles/voice/22-discuss.css` and
`tests/discuss_dock_browser.py`; mu-discussdrop owns
`web/src/view/catchup/panel.ts` (and its index only if routing needs it).
This lane touches none of those; its browser coverage is a new file so the
three lanes never serialise on one test file. The drop-to-partial runs on
the tap before the jump, and the expand runs inside the landing before the
scroll, so a Discuss citation lands expanded at the partial height.
