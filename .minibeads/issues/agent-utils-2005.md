---
title: 'vibe-talk-documents-keep-place: toggling away and back restores the open document and the list exactly'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-20T17:25:00-05:00
updated_at: 2026-11-21T00:58:00-05:00
closed_at: 2026-11-21T00:58:00-05:00
---

# Description

[watermelon-full-preview] Owner 275 (verbatim): "Man, the documents view is currently terrible about losing my place. If I switch back to chat to tell the agent something by toggling from documents to channelview. When I toggle back to documents view I want to see the exact same thing on the screen at the same scroll position. If I'm working my way through a document I want to be able to pick right back up where I left off. Right now it exits the document entirely and goes back to the list of documents rather than keeping the previous document open. Not only that, it loses my place in the list as well jumping to a random position scrolled far up"

Why it exits the document today: entering Documents through the view switch calls `documentsView.show()` with no key, and `show()` unconditionally runs `toList()`, which clears the open chain, sets `readingNow` false and closes the reader — a return is treated as a fresh arrival on the list however the reader was left. Why the list jumps far up: the shell's `showView` keeps ONE raw `scrollTop` per view, so the place saved for "docs" when leaving is whichever surface was up (the reader's mid-document offset applied to the list, or vice versa); then the return's `list.refresh()` lands on a raw offset captured before its fetch, overwriting the list's own relative anchor (`#1908`) wherever card heights above the reader changed, and a list that never opened a document has no recorded place at all (`listPlace` is only captured when a document opens from the list).

# Design

- One shared documents view state, owned by `view/documents/index.ts` and kept in memory for the page session: the open chain (each entry's document key plus its reader anchor), the list anchor, and whether the view has ever been entered. The reader anchor is the paragraph (drawn block) at the top of the screen and its pixel offset from the scroller's top, never a bare `scrollTop` (a raw offset is kept only as the fallback where no geometry can be measured); the list anchor is the top card's key, its offset, the draw order for nearest-survivor fallback, and whether the list was parked at its bottom.
- Writers: the view's own scroll capture on the shared scroller (while the Documents pane is up) keeps both anchors current; opening a document captures the list anchor, opening the next document or going Back captures the reader anchor. The reader and the list only measure (`captureAnchor`/`anchorDelta` on the reader, `capturePlace`/`restorePlace` on the list, over shared pure math in `viewstate.ts`); every scroll write still goes through the view's scroll-owner dependencies. Intra-list anchors stay with their owners: the download-tap card hold and fresh-load foot (`#1999`/`#2002`, mu-docjump) and the filter/refresh anchor (`#1908`) are unchanged.
- Fresh versus return: the first entry ever (or an explicit `show(key)` open) is FRESH — the list lands at its bottom, a first-opened document at its top. Every later bare `show()` is a RETURN: an open document is still open at its anchor, a list-only view refreshes and lands on its anchor, and Back from a document after a toggle lands on the list anchor. The shell's per-view raw place still runs first on a switch; the view's anchor restore runs after it and wins.
- An active document read: today `showView` already stops a document read when leaving Documents for the channel or the call (the read's source leaves the screen), and this change keeps that exactly — the read does not survive the toggle today, and the restored reader comes back stopped, at the place it was left. No new stop is added: the return path never closes the reader.

# Acceptance Criteria

- On a fixture, open a document, scroll to its middle, toggle Documents -> channel -> Documents: the same document is open and its top paragraph is back within 3px, at phone 412 and desktop 1280. The same for the list with no document open, and for Back from a document after such a toggle (the list lands at its own prior place).
- A fresh load still lands at the bottom (`#2002` split: fresh = bottom, return = restore); a first open still lands at the document's top; Back still walks the chain one step at a time and stops the read of the document being left.
- Focused JS suites cover the return contract, the reader/list anchor math at its boundaries, and the updated switch test (the old "a view switch forgets the chain" test encodes the reported bug and is rewritten); existing documents suites stay green.
- Browser fixture test with screenshots before toggle / after return (light and dark, phone 412 and desktop 1280) with BUILD.txt, run on the host with browser libraries via the exact command handed to the coordinator (headless `pw.chromium.launch()`); never the live server.

# Notes

Owner-priority, fast-track (owner UI). Lane mu-docstate under coordb. Split with mu-docjump (#1999 download-tap jump, #2002 fresh load lands at the bottom): fresh = bottom (theirs), return = restore (this issue); coordination sent before touching list scroll code (agentctl messages to mu-docjump first, then mu-doctoolbar #2001 and mu-readstart #2003/#2004 — all queued pending against locked agents at send time, message ids in the lane handoff). Related: #1082 documents-back-chain, #1264 documents-scroll-landings, #1908 vibe-talk-documents-back-and-filter-place, #1251 scroll-single-owner.

## Resolution

[watermelon-full-preview] Landed on github/main as 7ff7217b (tree ef4d2c83, combined over #2003/#2004's Read start rule and verified there: documents JS suite, typecheck, scroll-writer-guard, and the keep-place/scroll/read/download-jump/fresh-load browser walks all green). One documents view state (paragraph anchor for the reader, card anchor for the list) is kept by the view and restored on return; first entry stays fresh at the bottom. Evidence: focused suites green at every gate; the final gate's only reds (send-flash, column-overscroll-1920) are green solo on the change and on untouched main. Coordb verified the browser walks at each SHA and deploys.
