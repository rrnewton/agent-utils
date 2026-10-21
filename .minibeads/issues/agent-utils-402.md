---
title: 'who-is-who-channel-box: Who is who back in Settings'' channel section, following its picker'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- settings
created_at: 2026-10-21T14:51:11.180407857+00:00
updated_at: 2026-10-21T14:51:11.180407857+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21: "I see the who is whose section moved under the messages pane. But I think it should stay under the channel section along with everything that varies depending on selecting the channel in the drop down." #349 channel-settings-all (46d92828) moved Who is who out of the per-channel box to Messages, where it follows the channel being read, because the box's picker gained All channels. The owner now wants it back in the box, following the drop-down.

# Design

The list sits in the channel box under Clear unread, as it did before #349, and lists the speakers of the channel the box's picker is on, redrawn by renderChannelBox. With All channels (or a service's All <service> channels) chosen it shows one line, 'Pick one channel to see who is who.', in place of the list; the owner may ask for it hidden instead, so that is one branch. A choice still follows the account into every channel. Removed from Messages; Help and README updated.

# Acceptance Criteria

Page tests that fail before the change: the list is in the channel box and not in Messages; it follows the box's picker (negative control: following the channel being read); All channels shows the hint and no rows (negative control: a list drawn for All). Screenshots of Settings at 1280x800 and 393x852 before and after. make validate passes.
