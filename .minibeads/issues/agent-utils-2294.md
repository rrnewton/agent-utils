---
title: 'vibe-talk-dark-composer-black: in dark mode the message box gets the same black as the message cards'
status: closed
priority: 1
issue_type: feature
assignee: cl-composerblack
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T10:20:00-05:00
updated_at: 2026-11-26T12:35:00-05:00
---

# Description

Owner request 385 (2026-11-26 10:13 AM ET), verbatim: "(Btw, I would like the black background to apply to the COMPOSE window too because I look at that a lot.)"

Since `#2154 vibe-talk-dark-true-black-cards` the message cards in dark are true black, but the message box (composer) where the owner types keeps its grey dark background.

# Design

- Dark theme only; light unchanged. The composer's background reuses the token the dark message cards use, never a new hard-coded colour, so the `#2155 vibe-talk-theme-settings-pane` editor can still override it.
- Cover the channel composer, and check every other text box the owner sees as a compose window (thread reply, side and reply composers, the Discuss "Ask about this" field); list which changed and which did not, and why.
- Text, placeholder, caret and border stay readable on black (contrast ratios stated in the resolution).
- The page backdrop is not touched (that is `#2197 vibe-talk-dark-pane-background-darker`).

# Acceptance Criteria

- In dark, the composer's computed background equals the message card's computed background; in light it is unchanged. A page check asserts both at phone and desktop sizes.
- Screenshot matrix (light and dark, phone 412 and desktop 1280, keyboard open and closed, Discuss open) plus before and after of the dark composer, each folder with BUILD.txt.
- Contrast of text, placeholder and border on the new background is reported.

# Notes

Owner-UI fast-track. Owning lane: cl-composerblack. Related: `#2154 vibe-talk-dark-true-black-cards` (the card black), `#2155 vibe-talk-theme-settings-pane`, `#2197 vibe-talk-dark-pane-background-darker` (the backdrop, separate).

## Resolution

Landed as 8a5bd50f on 2026-11-26 (12:25 PM ET), replayed from 230b4e32 (gated on base b81f7674). In dark, every box the reader writes in takes the message cards' own surface token `--theirs` (true black since `#2154`) through one rule in `vibe-talk/web/styles/voice/03-reply.css`, keyed on the painted theme (`:root[data-theme="dark"]`, so a forced Dark on a light system is covered):

- the channel composer (`#channel-compose-text`), the thread box (`#side-compose-text`), the call's composer (`#compose-text`), the reply in the stream (`#reply-text`), the quick reply (`#quick-reply-text`), and Discuss's "Ask about this" field (`#discuss-followup`, added at the liaison's call).
- Light is unchanged: each box keeps its own surface (`--panel`; Discuss's field `--row-hover`). In every premade dark theme `--theirs` is that theme's card surface, so a theme that repaints the cards repaints the boxes.
- One selector per line, never an `:is()` list: the page suite's rule matcher splits a selector list on every comma.

Contrast on the new black (dark, computed in Chromium): words 17.29:1 (was 13.43:1 on `--panel`, 11.56:1 on Discuss's field), placeholder 4.56:1 (was 3.54:1 and 3.05:1), border 1.61:1 (was 1.25:1 and 1.07:1), focus ring 21:1; the caret is the text colour.

Check: `vibe-talk/tests/dark_composer_black_browser.py` (step `vibe-talk.check.dark-composer-black-browser`): six boxes at 412x915 by touch and 1280x800, system and forced themes, at rest, focused and with the keyboard up; dark boxes equal a drawn card's background with words and placeholder at 4.5:1 or better, light boxes keep their surface. It fails on the page before the change.

Gates: the owner gate on 230b4e32 (ticket 1795709021447732535-bd3f3555) passed 217 of 219 steps; the two reds, `vibe-talk.check.documents-read-browser` (a 20 s wall-clock wait on the fake TTS sidecar, filed as `#2304`) and `vibe-talk.test.page-3` (a CPU timeout before any test ran), touched nothing in this change (the documents check passed alone 3 of 3 on the change and 3 of 3 on its base; the whole page suite, 2632 tests, passed alone on the change), and their rerun on the same tree (ticket 1795711757879701218-fa88a9f4) passed. An earlier gate on the five-box version (98ebe9ca) was cancelled while queued once the Discuss field was added. Evidence: screenshot matrix before and after (light and dark, phone 412 and desktop 1280, keyboard open and closed, Discuss open) plus the reply, call, draft and thread boxes, reviewed by the liaison, and re-shot from landed main after the landing.
