---
title: 'hidden-channels-fold: hidden channels folded at the foot of Settings'' channel section'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- settings
created_at: 2026-10-21T14:51:19.833543606+00:00
updated_at: 2026-10-21T14:51:19.833543606+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21: "That little section that shows old hidden channels. I think it should stay at the bottom of that pane and be collapsed by default. When I'm switching channels and seeing the interface update it seems invasive to have this bit in the middle that shows the old removed channels." The list of hidden configured channels (#199 removable-config-channels) sits in the middle of the channel box, between its buttons and the panels and Clear unread under them.

# Design

The list becomes its own group at the foot of the Channels section, after the channel box: a fold (#346 settings-collapsible's fold-toggle, listed in SETTINGS_FOLDS), folded on every load and kept as the reader leaves it while the page is open, as every Settings fold is. Its heading counts the channels, 'Hidden channels (3)'; the group is hidden while there are none. A picker change does not touch it, so nothing above it moves and a folded list stays folded.

# Acceptance Criteria

Page tests that fail before the change: the list is a fold at the foot of the section, folded on load, labelled with its count; a folded fold stays folded across a picker change; Show again still works from inside the fold. A real-Chromium check at 1280x800 and 393x852 that the fold is the last thing in the section and that opening it moves nothing above it. Screenshots before and after. make validate passes.
