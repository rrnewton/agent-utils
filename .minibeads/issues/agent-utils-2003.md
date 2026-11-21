---
title: 'vibe-talk-read-starts-on-screen: Read starts at the first thing on screen in both views'
status: closed
priority: 1
issue_type: feature
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

[watermelon-full-preview] Owner 272 (verbatim, 2026-11-20): "The read button on the documents view is having some problems. When I press read I only see the stop button, not the pause button. And clicking on the document itself in the first paragraph to start the reading does not do anything. If I press the redundant read button in the upper right of the screen then it starts playing immediately and the stop and pause buttons both appear on the lower bar. I wanted to eliminate this redundant read button. And I want our default read button to begin reading immediately in the document at the top rather than waiting for a click. On the normal channels view, there's a lot of different messages and it's good for us to pick where we start reading at least for now. But on the document view there's really only one possibility. Actually I take that back in both views. I would like the same rule. I would like pressing the read button to start immediately and start reading the first thing that's on the screen. But it should not change the scroll at all. It should just begin highlighting what is on the screen."

Folded reports: owner 273 (no reading highlight at all) and owner 274 (no two-level navigator in document mode) are tracked as #2004 vibe-talk-read-highlight-missing on this same lane; the liaison's single-cause hypothesis (bottom-bar Read not recognised as the open document's read) ties 272/273/274 together and is verified first under #2004.

# Design

- New rule, BOTH the channel views and the Documents reader: pressing the bottom-bar Read starts reading IMMEDIATELY at the first item visible on screen — the topmost message or paragraph whose visible part is meaningful (the threshold is defined and tested) — with NO scroll change at all, only the highlight. Stop AND Pause both appear on the bottom bar, as they do today after the upper-right button.
- Documents: remove the redundant upper-right Read button (`#doc-read`); keep its working start path as the bottom-bar Read's implementation where that is the one that works.
- Documents: a tap on a paragraph jumps the read there ONLY while a read is already active (owner rule #1543: a text click never starts the reader cold). Owner 272's "clicking the first paragraph does nothing" was a click AFTER pressing Read, i.e. the #2004 bug (`readingActive` false), not a cold-start ask. Keep the existing rule from #1959 (one click on a message while the reader is on jumps there and reads it) and owner 228's behaviour. (Liaison correction, 2026-11-20: an earlier draft of this issue said a click "starts or moves" reading; "starts" from cold is dropped.)
- Name why the Documents Read showed Stop but no Pause (the #2004 verdict names it; this issue carries the start-rule fix).
- First search the tracker and ai_docs for the existing reader start-point design and the Read/Pause/Stop state machine (#1959, #252, #426, #434, #442, #487, #554, #706, #886 mention reading); note which earlier decision this rule replaces and record that design note's status here.
- Conflicts: mu-sendflash (#1983) and mu-windowed (#1985) own channel scroll/anchor code; mu-docjump (#1999/#2002) owns the documents list scroll; mu-doctoolbar (#2001) owns the documents toolbar. Message them before touching shared files, and never move scroll.

# Acceptance Criteria

- Pressing the bottom-bar Read in either view starts reading at once at the first meaningful visible item, with `scrollTop` unchanged to the pixel and that first visible item highlighted; page tests pin this at phone and desktop sizes in both views.
- The upper-right Read button is gone from the Documents reader, and the bottom-bar Read takes its start path.
- A tap on any document paragraph, including the first, jumps reading there while a read is active; a cold tap (no read active) starts nothing (#1543).
- The Pause button is present whenever reading (with Stop beside it).
- Screenshot matrix (light and dark, phone 412 and desktop 1280, plus Discuss open on phone) of before and after pressing Read in both views, BUILD.txt in every folder; the exact browser-host command is given (headless shell, `pw.chromium.launch()`).

# Notes

[watermelon-full-preview] mu-readstart. Owner-priority (owner UI fast-track). Related: #2004 vibe-talk-read-highlight-missing (verify first; this issue builds on a working highlight), #1959 vibe-talk-reader-one-click-jump, #1541, #1330, #1543, #1505. Test instances only; never the live instance (127.0.0.1:18090 or 8422).

Earlier decision replaced (tracker search, 2026-11-20): the standing start-point rule is "reading mode waits for a tap" — entering the mode only arms it and prepares speech, with the status "tap a message to hear it" (`view/voice/readingacts.ts`, `setReadingMode`), and a row tap then starts the read. This issue replaces that wait with an immediate start at the first visible item in both views. Kept, not replaced: #434 (Read is the playing read's Pause/Resume, one state path), #442 (Stop stands beside Pause while a read plays), #1543 (a text tap never starts cold, only jumps an active read), and #1959/owner 228 (one click on a message link while the reader is on lands expanded and reads it).

## Resolution

[watermelon-full-preview] Landed on main as 47454e8a (replay of 154c6ed3 over f65e8a9a, verified tree e6e6fc14). Pressing the bottom-bar Read now starts immediately at the first meaningful visible item (40px, or half of a short item) with no scroll change, in both views; the header Read button is gone; with Discuss open the press arms only and the panel starts the summary. Cold taps still start nothing (#1543). The gate on the squashed tip had only load/timing reds (read-aloud-blocks and backfilter green solo; read-pause #2020; picker and screenshots fixed on main), waived by the coordinator; the combined tree over #1997 was verified (Rust documents/http tests, clippy, fmt, typecheck, documents JS, and the documents/views/GitHub-helper browser checks) before landing.
