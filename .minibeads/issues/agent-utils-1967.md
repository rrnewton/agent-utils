---
title: 'vibe-talk-reader-pause-on-any-composer: pause the read on any path into a reply composer'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-aloud
created_at: 2026-11-20T10:34:00-05:00
updated_at: 2026-11-20T13:21:00-05:00
closed_at: 2026-11-20T13:21:00-05:00
---

# Description

[watermelon-full-preview] Owner request 229 (2026-11-20 10:34 AM ET), owner-UI bug, fast-track, verbatim: "Okay, I was using Reed mode in the car as usual and ran into some glitches. There was another situation where I tried to react. With the reaction button. Or maybe I also press the quote reply button at some point. But I ended up into a quote. Reply dialogue I think. Docked and it was still reading out loud and transcribing its own words into the quoted reply. That is a big glitch. We need to have an ironclad rule that whenever we get the cursor into a reply dialogue. Whether from the reply button, the quote, reply button or for me pressing the react button on the reader. Whatever the path we need to pause the active reading. When I press the react button I also like it to immediately start transcribing my reply. Then when I send my reply it should be a simple resume to keep playing. But irrespective of whether listening is turned on automatically on a given path, we need the act of reading to always be paused irrespective of what code path and uipath we take"

Invariant: while the reader is reading (device voice OR agent voice), ANY way of putting focus or dictation into a reply composer pauses reading BEFORE the mic can hear anything. That covers the Reply button, Quote reply, the reader's React button, the docked composer, keyboard focus or shortcuts, a tap into the textarea, and any other path. The reader's own speech must NEVER be transcribed into a draft. React additionally starts dictation immediately. Send resumes reading exactly where it paused (no restart from block 1). Cancel or close leaves reading paused, unless the existing code resumes, in which case keep existing behaviour and say so.

Gap found in the current page: React while playing already pauses first (`reactToHeldSentence`), but the row Reply / Quote reply button (`openReply`, docked `#reply-text`), the selection chip and its strip twin (`pressSelectionChip` -> `openQuickReplyFor`), the Draft chip (`openQuickDraft`), the dock icon (`dockQuickReply` -> docked reply), keyboard shortcuts that focus `#reply-text`, and a direct tap into either reply textarea all put the cursor in a reply composer with no pause. The existing browser check (`vibe-talk.check.read-pause-reply-browser`, #554/#819/#821/#1250) always pauses manually before Reply/Quote, so the while-playing docked and chip paths slipped past it.

# Design

Enforce at ONE choke point, not per button:

- One document `focusin` listener (in the read controls' listeners, `mountReadControls`) that pauses the read whenever focus lands in a reply composer: `#reply-text` (the docked/in-stream reply, moved between the column and side reply hosts) or `#quick-reply-text` (the floating quick reply). Every path above ends in a focus on one of those two, so new paths are caught by construction.
- One guard inside dictation start (`createDictation`'s session launch, which the initial start and every #1503 restart go through): pause the read before the recognition starts, so the mic can never hear the reader.
- The read owner (`createReadAloud`) gains `pauseForComposer()` (pauses only a playing read, and remembers that the composer held it), `resumeComposerHeldRead()` (a Send from such a composer carries the read on from where it was held), and `clearComposerHeldRead()`-equivalent clearing on any manual resume/stop. A read the reader had already paused by hand before opening the reply is NOT auto-resumed on Send: the #554 pause, Reply, Send, Resume-tap flow keeps its existing behaviour.
- React keeps its existing pause-first-then-dictate behaviour, now also marked composer-held so its Send resumes.

Cancel or close leaves reading paused (existing behaviour, kept and stated here).

# Acceptance Criteria

- Module tests: dictation pauses before the first and every restarted recognition session starts; the read-controls focusin listener pauses only for the two reply composers and for no other focus; the read owner's composer-held flag resumes on the composer's Send and on nothing else (not on Cancel/close, not where the reader had paused by hand first).
- The browser check `vibe-talk.check.read-pause-reply-browser` is extended to ENUMERATE every path into a composer while reading, for both voices where the path exists: row Reply, row Quote reply (selection held), selection chip, strip Quote reply, React, docked-from-floating, Draft chip, keyboard focus, and a direct tap into each textarea. Each asserts the read is paused (Read says Resume, no further utterance/clock movement) while the composer is focused, that no reader speech lands in the draft, and that Send from a composer-held pause resumes from the held sentence/block with no restart from block 1. Behaviour-only verdicts, condition-based waits (owner rule 346).
- e2e through the REAL router on a fixture (device voice and agent voice), phone and desktop, logging tap-to-pause timing; screenshot matrix per the standing owner-UI acceptance on a local test instance only (never the live server).
- Focused suites green locally; one owner-priority gate; landed on main.

# Notes

Owner UI fast-track (owner-priority gate, deployed on landing). Lane mu-pausecompose from coordb. Prior work: #554 read-pause-reply-resume, #547 car mode, #1250 (browser-check robustness rules), #1503, #821, #828. #297 is unrelated (cache races), noted as checked.

## Resolution

[watermelon-full-preview] Landed 2026-11-20 13:21 ET as e9ae38cd on github/main (replayed 0e5b7d85 onto 64b22634; coordb waiver for devbig014's missing Chromium library). Built as designed: pauseForComposer choke point + one document focusin in the read controls + guard inside dictation start; Send from a composer-held pause resumes from the held place, hand-paused reads keep the #554 flow, Cancel/close leaves the read paused. Module tests green; whole browser walk green on devbig030 (every enumerated path paused while playing at tap-to-pause 25-91ms; Send resumed from the held place at phone and desktop, both voices); the screenshot matrix reviewed and approved by coordb.
