---
title: 'vibe-talk-linked-bubble-follows-back: after Back from a citation jump the Linked message bubble returns to the row the reader came back to'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
- ui
created_at: 2026-11-25T17:56:00-05:00
updated_at: 2026-11-25T17:56:00-05:00
---

# Description

In All with Discuss open, a reader taps a summary citation to a main-channel message: that row lands with the "Linked message" bubble. They then tap a second citation, to a thread reply, and the reply lands with the bubble. Back puts the reader back on the first message at the height it stood (#2261 vibe-talk-summary-link-back-race), but the bubble stays on the reply row, which is now off screen, and the row the reader came back to stands without its bubble, shorter than it was.

Seen in the #2261 evidence, the same on main before and after that fix: the before-tap frame shows the first message with its bubble, and the after-Back frame shows the same row at the same top with no bubble. The row's place is right; its mark is not. Back is meant to return the reader to the view as it was, and the bubble is part of that view.

# Design

- On a return from a link jump (`returnToLinkSpot`), put the linked mark back as it stood when the jump was taken: the spot already keeps the deep-link focus (`spot.focus`) and restores it with `deepLinks.setFocus`, but the drawn mark (`setLinkedMark`) is left on the jump's target. Restore the mark from the spot (or clear it where the spot had none), in the same task as the restore, through the existing linked-mark owner.
- The bubble's height comes back with it. That change sits below the row's top, so the restored top is unchanged. Hold the row through the scroll owner as the return already does, and check that the per-frame Back trace (#2261) still reads within 0.5 px.
- A reader who dismissed the bubble (its x) before the jump gets no bubble back: the spot records that the mark was out.

# Acceptance Criteria

- After Back from the reply-citation jump in the summary-link walk, the row the reader came back to carries the "Linked message" bubble and the reply row does not, at 412x915 and 1280x800, light and dark.
- The per-frame Back trace still shows the row within 0.5 px of its pre-jump top on the first frame drawn after Back and on the frames after it.
- A mark dismissed before the jump is not brought back by Back.
- Before and after screenshots at the same moments as the #2261 pair (before the tap, after Back), with the row's text and rect.top in a sidecar, taken on a local test instance.

# Notes

Filed at the liaison's request from the #2261 review (2026-11-25). A small owner-UI follow-up, not fast-tracked.
