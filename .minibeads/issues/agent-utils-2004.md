---
title: 'vibe-talk-read-highlight-missing: no reading highlight or navigator while a document read plays'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- reader
- documents
- ui
assignee: mu-readstart
created_at: 2026-11-20T17:03:20-05:00
updated_at: 2026-11-20T17:03:20-05:00
---

# Description

[watermelon-full-preview] Owner 273 (2026-11-20): "right now the arrow keys are working for navigating chunk to chunk and skipping to the next chunk as replay proceeds. But the overlay that highlights what's being read isn't working at all. I see no highlight". Owner 274 (same day): "I also don't see any two level navigator bar when reading in document mode. Why was this dropped".

Folded with owner 272 (see #2003 vibe-talk-read-starts-on-screen): on the Documents view the bottom-bar Read shows Stop but no Pause, a tap on the first paragraph does nothing, and only the redundant upper-right Read starts playback at once with Stop and Pause both on the bottom bar.

Liaison hypothesis to verify first (coordb, 2026-11-20): a read started from the bottom-bar Read is not recognised as the open document's read, so the Documents reader's `readingActive` stays false. That one fact would give all three reports at once — no navigator (shown only while `readingActive`, #1541), no highlight (lit only from the document read's `onReadingBlock` report, #1330), paragraph taps ignored (the #1543 gate), and Stop without Pause (the bottom bar is in channel reading mode with nothing playing). The upper-right Read starts the read the "right" way and shows none of this. Check channels too: the same "no highlight" may or may not hold there.

# Design

- Verify the hypothesis before any bisect, on a fixture with a fake audio/TTS path (local server only, never the live instance): start a document read from each button in turn and log `readingActive`, the scrubber/navigator (`#doc-nav`) presence, the highlight element (`[data-reading-block="current"]` / `::highlight(read-aloud-sentence)`), the Pause button, and a paragraph tap. Send coordb the verdict (holds / does not, with the logged states).
- If it holds, the fix is that the bottom-bar Read becomes the document's read (the #2003 fix), landed as its own commit here: while a document is open, pressing the bottom-bar Read starts that document's read through the same `readDocument` call the upper-right button makes, so `readingActive`, the navigator, the highlight, the tap gate and Pause all follow from the one read. The channel views keep today's tap-to-choose behaviour until #2003's first-visible rule lands there.
- If it does not hold, bisect the day's deploys (live 353412fc; chain in the lane brief) across the suspects #1961 colour themes and the #1959/#1967 reader changes: highlight present or absent during playback in Documents and channels, light and dark, each colour theme. Name the breaking commit and the cause, then fix that as its own commit.
- Either way the fix carries a test asserting the highlight element is visible (non-zero box, colour distinct from the background) while a chunk plays, in every colour theme, and screenshots of the highlight visible during playback (phone 412 and desktop 1280, light and dark) with BUILD.txt in every folder.

# Acceptance Criteria

- The logged-states verdict (bottom-bar vs upper-right start) is recorded on this issue.
- While a document chunk plays, the block being read carries a visible highlight (non-zero box, colour distinct from its background) in Documents, in every colour theme, light and dark; the same check passes in the channel views or the channel gap is named.
- The two-level navigator (`#doc-nav` in the read scrubber) is visible while a document read stands, from either Read button.
- The Pause control is present whenever a read is playing (see also #2003).
- Screenshots of the highlight during playback (phone 412 + desktop 1280, light + dark) with BUILD.txt are delivered to coordb; the exact browser-host command is given (headless shell, `pw.chromium.launch()`).

# Notes

[watermelon-full-preview] mu-readstart. Owner-priority. Related: #2003 vibe-talk-read-starts-on-screen (the first-visible start rule and the upper-right button's removal, which follows naturally once the bottom-bar Read is the document's read), #1541 documents-nav-outline-tree, #1330 doc-read-highlight, #1543, #1505 documents-read-controls-highlight-tap, #1961 vibe-talk-colour-themes, #1959 vibe-talk-reader-one-click-jump, #1967 vibe-talk-reader-pause-on-any-composer. Test instances only; never the live instance (127.0.0.1:18090 or 8422).
