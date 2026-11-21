---
title: 'vibe-talk-hamburger-reader-group-right-hand: speaker before Reader, Hide without the repeated word, Autoplay first, menu rows right-justified'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- ui
created_at: 2026-11-21T10:20:00-05:00
updated_at: 2026-11-21T10:20:00-05:00
---

# Description

Owner request (2026-11-21 10:20 AM ET), quoted in full:

"On mobile when I pop up at the hamburger menu I see the reader options grouped. I would like it to have the speaker symbol before the word reader to make it a little clear that it's the audio reader and I would like to remove the redundant word reader from the hide reader button. It's already under the reader grouping so we don't need the reader word twice. I would also like to flip the order so autoplays first and hide is on the right. I would also like to write justify any menu buttons in this hamburger menu bar. If it occupies two rows, I would rather the second Roby right indented so I can reach it with my thumb. Later on we can potentially have a left-handed mode where we document all of the places we bias towards the right and we flip them to the left. But right now we don't have other users and I want to bake in the right-handed behaviors"

In vibe-talk's hamburger menu (`#action-menu` in vibe-talk/web):

1. The Reader group label carries the speaker glyph the page already uses for sound/read-aloud (the `#read-aloud` button's speaker-and-waves paths), before the word "Reader", so the group reads as the audio reader.
2. The "Hide reader" button's visible word is "Hide"; its accessible name (`aria-label`) and `title` keep the full "Hide reader".
3. Within the Reader group, Autoplay comes first and Hide stands on the right.
4. Every button row in the hamburger menu is right-justified; a group that wraps to two rows keeps its second row right-aligned too (thumb reach), never left.
5. Every right-hand bias is written down (Notes below, and a comment at the `#action-menu` layout rule in 18-control-bar.css) so a later left-handed mode can flip them. No left-handed mode is built now.

# Acceptance Criteria

- Phone (412px) and desktop (1280px), light and dark, menu open: the Reader group shows speaker + "Reader", "Autoplay" then "Hide", rows ending at the right edge; plus one shot where a group wraps to two rows with the second row right-aligned.
- Page typecheck passes; generated ids regenerate with no diff; the page shards covering the menu and the menu-related browser checks pass (local fixture server only, never the live server).

# Notes

[watermelon-full-preview] Filed from the owner's words above (relayed by coordb, 2026-11-21). Page-only change in vibe-talk/web; no left-handed mode.

Right-hand biases (flip all of these for a future left-handed mode):

- (new, #2052) Every `#action-menu` row is right-justified (`justify-content: flex-end` on `#action-menu`); a wrapped second row is right-aligned too, never spilling back to the left.
- (new, #2052) Inside the Reader box, Hide stands at the right end, after Autoplay — the thumb edge holds the action that takes the reader row away.
- (existing) The hamburger opener stands immediately left of the view switch, and the view switch is the bar's last button at the lower right in every view (`#1078 bottom-bar-switcher`), so the menu is opened from the right edge.
- (existing) In the reader's own row, Read keeps the right edge and Stop stands on its left (`#442 read-strip-order`), so the Pause thumb-place never moves.

The same list is recorded as a comment at the `#action-menu` rule in vibe-talk/web/styles/voice/18-control-bar.css. The prompts tray is not the hamburger menu and keeps its left start; it is not a bias to flip with this menu.
