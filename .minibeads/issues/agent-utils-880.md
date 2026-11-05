---
title: 'voice-page-menu-placement: the hamburger sits left of the view toggle, and its menu hides or shows the reader row'
status: closed
priority: 0
issue_type: feature
assignee: cl-refactor
labels:
- vibe-talk
created_at: 2026-11-05T10:54:21.325423710+00:00
updated_at: 2026-11-05T12:08:54.140099731+00:00
closed_at: 2026-11-05T12:08:54.140099611+00:00
---

# Description

[opus 5.5] OWNER REQUEST (2026-11-05, 05:53 AM ET), ASAP. The owner's words, verbatim: "Originally I asked for this hamburger icon just to the left of the channel voice view toggle. Please fix. Also give me an option under hamburger to hide/show reader. Is hide the big bottom row of blue buttons. Please implement asap and redeploy so I can use this in vibe talk"

This follows #820 voice-page-action-menu, whose design placed the hamburger "just LEFT of the channel selector". The owner now wants it immediately left of the Channel/Voice view toggle. It is an owner-approved exception between #624 voice-page-modularity batches.

# Design

1. Move the hamburger/menu button so that it sits immediately left of the Channel/Voice view toggle, on phone and desktop layouts.
2. Add a menu item, "Hide reader" / "Show reader" (the label reflects the current state). It hides or shows the bottom row of large blue reader buttons.
   - Persist the choice across reloads in localStorage, as the other page preferences are. Default: shown.
   - Hiding keeps the reader's keyboard shortcuts working and leaves no empty gap.

# Acceptance Criteria

- In the DOM and visually, the menu button directly precedes the view toggle on both layouts.
- The menu item toggles the reader row and its own label.
- The choice survives a reload, and the default is shown.
- With the row hidden, the reader shortcuts still work and no gap remains.
- The browser and layout tests that pinned the old position are updated. Gate, land, and send the SHA to the lead for an immediate deploy.

# Notes

Relayed by the lead on 2026-11-05. Assigned to cl-refactor, ahead of #624 batch 62.

## Resolution

[opus 5.5 cl-refactor] Landed as 2b9ae17e on github/main (gate 131/131, adversarial review recorded). The hamburger
left the scrolling pack and sits immediately left of the Voice/Channel button on a phone, with the bar in the header,
and on a desk (in the corner beside the switch). The action menu's "Hide reader" / "Show reader" item, on the channel's
view, hides the row of big reader buttons with no band left, is kept on the device (`vibe-talk.voice.reader-hidden`,
default shown), and leaves h and s working. The row is still drawn while Hang up, Stop or a lit Read is present, so a
playing read or a live call is never left without a way to stop it. Unverified: the owner's own phone after the deploy.
