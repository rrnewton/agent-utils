---
title: 'discuss-top-bar-slim-fixed: remove the Discuss top-bar Read button; fixed slim "Discuss agent" top bar'
status: closed
priority: 1
issue_type: feature
assignee: mu-discusstop
labels:
- vibe-talk
created_at: 2026-11-14T15:51:00.000000000+00:00
updated_at: 2026-11-14T15:51:00.000000000+00:00
---

# Description

[opus 5] Owner UI request 059(b)+(c) (2026-11-14, screenshot of the phone Discuss pane): the Discuss pane's top bar has a big "Read" button, a maximize button and an X. (b) The Read button is redundant: the page's own reader controls (bottom toolbar Read/Stop, speed) should read the summary. (c) The top bar (maximize and X) should stay fixed while the pane's content scrolls, carry the title "Discuss agent", in a font small enough that the bar is no taller than its buttons, on phone and desktop.

# Design

- `#1531` owns `.discuss-head` only: `.discuss-bound` stays with mu-hoursbound and the summary fold markup with mu-foldline; mu-sumbar owns the `.discuss-ask` placement (bottom bar on phones) and mu-discussdock the phone docking.
- The `#discuss-read` button leaves `voice.html`, `panel.ts` and the generated id map (`ids.ts`, held exact against the markup by the contract test). The panel's whole-summary `readAloud` stays for tap-to-read and gains a `readSummary()` entry on the panel and the Discuss view.
- The top-bar button was the only one-tap whole-summary path, so the panel now listens to the bottom toolbar's `#read-aloud` itself (no page-script wiring changes): with Discuss open and a summary on screen it starts the whole-summary read through the shared owner, unless this summary's read already stands (that press is its Pause) or another read stands (its strip is up). Stop, Pause and pace beside it remain the shared owner's own controls.
- `.discuss-head` becomes sticky at the panel's top, full-bleed with its own background and bottom border, holding the small title "Discuss agent" (0.85rem) and the dock/fullscreen/close icon buttons. Phone icon buttons are 44px (2.75rem) tap targets, so the title row is 44px plus the bar's padding; desktop keeps 32px buttons.

# Acceptance Criteria

- No Read button in the Discuss top bar; the bottom toolbar's Read reads the summary whole, a second press does not restart a standing read, and Stop/pace remain the shared owner's.
- The top bar stays at the panel's top while the pane's content scrolls, shows "Discuss agent", and its title is no taller than its buttons; phone buttons are at least 44px.
- Unit tests cover the toolbar Read path and the standing-read guard; the Discuss browser walk covers no-top-bar-Read, the title, button size and the fixed bar while scrolling at 390px, 393px and desktop widths, with condition waits only.

# Notes

Owner UI fast-track (owner rule 286), request 059(b)+(c), one bar and one commit so (b) and (c) land together. Discuss read-aloud parity from `#1501`; sibling lanes mu-hoursbound, mu-foldline, mu-discussdock, mu-sumbar and mu-discussread land on their own; whoever lands second rebases and re-gates. Gate: owner tier (GATE_PRIORITY=owner), then land-main and coordb deploys.

## Resolution

The Discuss top bar no longer carries a Read button: the bottom toolbar's Read now reads the Discuss summary whole through the shared read-aloud owner (the panel wires that toolbar button itself, guarding a standing read so the press stays its Pause), with Stop and pace unchanged beside it. The top bar is a slim sticky bar at the pane's top carrying "Discuss agent" in small text beside the dock, maximize and close buttons, 44px tap targets on phone and 32px on desktop, while the pane's content scrolls under it.
