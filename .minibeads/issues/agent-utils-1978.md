---
title: 'vibe-talk-cmd-d-dock-toggle: Cmd+D toggles dock/float for the focused quote-reply pop-up or Discuss pane'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- keyboard
created_at: 2026-11-20T13:36:00-05:00
updated_at: 2026-11-20T13:36:00-05:00
---

# Description

[watermelon-full-preview] Owner request 251 (2026-11-20 1:15 PM ET), owner-UI fast-track, verbatim: "Hmm I'm feeling like I would like Cmd-d to mean "dock" both if I'm in a pop-up quote reply and if I have a floating discuss pane focused... any other conventions from vscode or whatever that would be relevant for that?"

Owner update 252 (2026-11-20 1:23 PM ET) overrides the original brief's Shift fallback: "ok, let's stick with plain cmd-d and make it so". Plain Cmd+D only on Mac; no Cmd+Shift+D variant. Off Mac use plain Ctrl+D (never bare Ctrl+D on Mac).

# Design

- Plain Cmd+D (Mac) / plain Ctrl+D (off Mac) toggles dock/float only when focus is inside one of two surfaces, their composers included: the floating quote-reply pop-up (`#quick-reply`, or the in-stream reply it docked into, `#reply-compose`) or the Discuss pane (`#discuss-panel`). It calls preventDefault only when it acts; everywhere else the chord does nothing new and keeps its browser default.
- Quote reply: floating to docked is the existing dock act (draft, quote and Post to main move into the in-stream reply, focused); docked to floating floats a quoted reply back out as the floating box, carrying its words past the quote and its Post to main choice, with the draft living in the floating box's own draft store afterwards. A plain in-stream reply (no quote at its head) has no floating form and does not move.
- Discuss: floating (`overlay`) to docked and docked to floating follows the header dock toggle's semantics, including leaving a maximized pane to docked.
- No Shift variant is bound. Bare Ctrl+D on a Mac is never taken (it stays forward-delete in a text field). Esc behaviour is unchanged.
- Both dock controls name the chord (`aria-keyshortcuts` via the keymap's `controls`), and the binding is listed in the list of keys / help in `keymap.ts`.
- The owner's "other conventions" question is answered with suggestions only in the handoff; nothing extra is built.

# Acceptance Criteria

- A page test through the page's real keyboard dispatch on the fixture covers both surfaces, docked to floating and floating to docked both ways, with focus inside the composer, plus a negative case (the chord outside the surfaces does nothing and is not swallowed), and the no-Shift-variant and never-bare-Ctrl+D-on-Mac cases.
- Before reporting done, the feature is exercised dynamically on a local test instance from the worktree build (never the live instance), with the command, output and screenshots as evidence; screenshot and browser checks that need a browser run on the host that has one, via the exact commands sent to the coordinator.
- Focused suites for the touched areas pass; one gate on the final SHA, then land to `main` directly per this repository's publication policy.

# Notes

Number reserved by the lead coordinator (coordb ledger) for this lane, confirmed 2026-11-20. Related: `#1315 floating-reply-dock`, `#1354 discuss-layout-modes`, `#889 voice-page-copy-link-key` (the Cmd+L / Cmd+Shift+L pattern the original brief cited; superseded for this binding by owner update 252), `#1386 discuss-mod8-toggle`.
