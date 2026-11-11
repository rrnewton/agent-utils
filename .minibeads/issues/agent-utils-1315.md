---
title: 'floating-reply-dock: a dock icon on the floating reply box turns it back into the inline reply editor'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-11T16:09:08+00:00
updated_at: 2026-11-11T16:43:07+00:00
closed_at: 2026-11-11T16:43:07+00:00
---

# Description

[opus 5.5] Owner request (reply 379): "When I press q it currently pops out the floating reply dialogue. That's fine but if I change my mind I want the lower left corner of it to be a dock icon that turns it back into the inline reply-after-message UI."

Related: #1034 quote-reply-key, #727 floating quick-reply, #410 inline-reply-editor.

# Design

- A dock icon at the floating reply box's lower-left corner, with a tooltip and an accessible label.
- Clicking it moves the draft into the inline reply editor under the message it answers: the text, the quote, any attachments and the post-to-main state, with focus kept and nothing lost. The floating box closes.
- Any scroll that brings the inline editor into view goes through the single scroll owner (#1251 scroll-single-owner).
- Owner fast-track: path-selected gate, deployed on landing.

# Acceptance Criteria

- A desktop browser walk: press q, type a draft with a quote, click the dock icon; the inline editor under the message holds the same draft, has focus, and the floating box is gone.
- The same walk with post-to-main ticked keeps it ticked after docking.
- No wall-clock waits in the test (owner rule 346).

# Notes

## Resolution

[watermelon-full-preview mu-dock] The floating reply box carries a dock icon at its lower-left
(`#quick-reply-dock` in the actions row, tooltip and aria-label "Dock this reply under its
message"). Clicking it (`dockQuickReply` in `web/src/view/channel/quoting.ts`) moves the quote
block and the written text — the quote alone when nothing is written yet — into the message's
reply draft and opens the inline reply under that message, focused at the end of the draft, with
the floating box closed and holding nothing for those words. The floating box has no attachment
control, so there is no attachment state to carry. Its Post to main tick is carried explicitly:
`quickReplyToStream` in `web/src/view/channel/replysession.ts` (the only replysession function
touched; `sendReply`, `leaveDockedReply` and `holdRowAt` untouched) re-ticks `#reply-to-main`
after the open and re-says the destination. A tap outside keeps its old behaviour, tick included.
Bringing the inline editor into view uses the existing `openReply`/`dockReply` path; no new
direct `scrollTop` write was added. Adding the button's id regenerated
`web/src/view/dom/ids.ts` via `node web/ids.mjs --write`.

Evidence: page suite `tests/js/page/replies.mjs` (197 tests, including the new `#1315` test at a
desk and on a phone, empty-dock, and post-to-main docked then sent), `tests/js/view/composer.test.mjs`
and `channel.test.mjs` (96), `tests/js/page_build.test.mjs` + `contract.test.mjs` (41),
`make typecheck`, strict mypy on the browser test, and the desktop 1280x800 Chromium walk in
`tests/reply_continuity_browser.py` (`1280x800-dock-reply`: q, quoted draft, dock, focused inline
editor on screen, sent from there; `1280x800-post-to-main` keeps the tick through docking and
sends `post_to_main`). The full browser file passes; a local `make validate` run's only failure
was the stale generated-ids file, fixed by the regeneration above and re-verified in the two
suites that named it.
