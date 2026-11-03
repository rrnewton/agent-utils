---
title: 'voice-page-quoted-reply-and-draft-chip: the quoted-reply button always works; a Draft chip finds every unsent draft'
status: closed
priority: 0
issue_type: bug
assignee: cl-refactor@devbig030
labels:
- vibe-talk
created_at: 2026-11-03T15:53:06.387811132+00:00
updated_at: 2026-11-03T18:14:25.295348515+00:00
closed_at: 2026-11-03T18:14:25.295348395+00:00
---

# Description

[opus 5.5] OWNER P0 (2026-11-03), part 1 of 2, most urgent: the owner lost a dictated reply. Owner-approved exception to the #624 voice-page-modularity linearization rule: this page feature work lands while #624 is paused.
(a) BUG: the inline quoted-reply button on an inline selection sometimes cannot be pressed, for example in Reading mode with the reader paused; it behaves as if something floating covers it.
(b) NEW: a floating "Draft" chip, so an unsent reply is never lost from sight.

# Design

(a) The quoted-reply button must ALWAYS be pressable: check z-index, pointer-events and overlay hit-testing against every floating layer (reader controls, chips, panels). The text box it opens must accept typing and speech.
(b) A prominent floating "Draft" chip shows whenever any draft exists anywhere in the current channel: quoted-reply drafts, thread replies, the main composer. Tapping it jumps to the draft and opens or focuses it; with several drafts it cycles, newest first. Builds on the #727 draft persistence: a draft is never lost.
Mobile first.

# Acceptance Criteria

- A browser test presses the quoted-reply button in Reading mode with the reader paused, on phone and desktop layouts, and types into the box that opens; a hit-test assertion shows no layer covers the button.
- Speech input reaches the opened box.
- The Draft chip appears for a quoted-reply draft, a thread-reply draft and a composer draft; tapping it focuses each, newest first, cycling with several.
- Drafts survive reload (via #727).
- Landed and deployed separately from part 2 (#820).
