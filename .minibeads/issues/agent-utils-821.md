---
title: 'voice-page-read-aloud-react: pause read-aloud on a sentence and reply to exactly that sentence by voice'
status: open
priority: 1
issue_type: feature
assignee: cl-refactor@devbig030
labels:
- vibe-talk
depends_on:
  agent-utils-820: blocks
created_at: 2026-11-03T15:55:39.676569469+00:00
updated_at: 2026-11-03T15:55:39.676569469+00:00
---

# Description

[opus 5.5] OWNER request (2026-11-03), the next page feature after #819 and #820: react to the sentence being read aloud, without hunting for it. While read-aloud speaks, the owner wants to pause on a sentence and reply to exactly that sentence by voice, on the go (Android is primary). Owner-approved page work; it follows the #819/#820 exception to the #624 linearization rule.

# Design

- While read-aloud is speaking, the current sentence or bullet is highlighted. When the owner presses Pause, a REACT button appears beside Pause and Stop.
- Layout: the existing left/right navigation row becomes more centered, with React at the far right of that row.
- The highlight HOLDS on the spoken sentence while paused.
- Pressing React opens an inline quoted reply on exactly that sentence, reusing #819's always-pressable quoted reply and its Draft chip, so the reply is never lost.
- Focus goes into the reply box with the cursor inside. Ideally voice input starts at once so the owner can just speak. A web page cannot switch the Android keyboard into voice mode, so the options are: (a) start in-page dictation automatically on React (Chrome's webkitSpeechRecognition, or the suite's own ASR stack) with a visible microphone state; or (b) only focus the box and let the owner tap the keyboard's microphone. Prefer (a) if it is reliable, with (b) as the fallback.
- A design note comes first (mu-transcript@devbig014, design only, no voice.js edits): where the read-aloud highlight state lives and how it holds on pause, the navigation-row layout, a dictation feasibility test on Android Chrome, and the reuse points with #819. The build follows it.
- Owner addition (2026-11-03): in Reading mode on mobile the forward/back navigation buttons get BIGGER touch targets (at least about 48 px, e.g. 52-56 px), still fitting the centred row with React at the far right. It may ship with #820 part 2 instead if that is simpler; record which.

# Acceptance Criteria

- Browser test: during read-aloud the current sentence is highlighted; after Pause the highlight stays and React appears at the far right of the centred navigation row, on phone and desktop layouts.
- React opens a quoted reply on exactly the highlighted sentence, with the cursor in the box; the draft shows in the Draft chip.
- On Android Chrome, dictation starts automatically with a visible microphone state when supported; otherwise the box is focused for the keyboard microphone.
- The design note is linked from this issue.
- In Reading mode on a phone layout the forward/back buttons measure at least 48 px (target 52-56 px) and the row still fits with React.
