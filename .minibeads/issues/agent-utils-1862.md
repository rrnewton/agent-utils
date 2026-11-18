---
title: 'vibe-talk-floaters-off-text: the remaining floaters never cover the message being read'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
created_at: 2026-11-18T16:45:00-05:00
updated_at: 2026-11-18T16:45:00-05:00
---

# Description

[watermelon-full-preview] Follow-up to #1859 from the liaison's review: the other floaters still cover message text — the "Reading with Device voice." notice, the Auto/React chips during read-aloud in the channel view, the "Live · updated" pill, and an action notice truncated on a phone covering the first card. The desktop reading strip's overlap of the lit sentence is #1866 (the follow owner's lane) and is out of scope here.

# Design

- The current freshness pill ("Live · updated …") is idle information like "Ready.": while what it says is current it carries `data-idle` and is waited out (announced, not floated over the first card). Warnings — stale, offline, slow, failed — draw as before.
- The read-aloud device-voice confirmations ("Reading with the device voice." / "Reading with Device voice.") join the status line's idle confirmations: announced, not floated over the foot of the list.
- A strip standing for React alone keeps its place, and the scrollers keep scroll room at their foot while it stands, so a landing never puts the sentence being read under it. No follow-code change (that is #1866's).

# Acceptance Criteria

- While reading, no listed floater covers the message being read, at phone or desktop; warnings and action notices still draw.
- Deterministic tests (condition waits only), a browser check that the current freshness pill and the reading notice are not drawn over text while their announcements stand, and the screenshot matrix (light/dark, keyboard open/closed, phone 412x915 and desktop 1280x800, Discuss half-pane on phone) captured on a test instance and inspected.

# Notes

Owner-priority, deployed on landing. Builds on #1859 vibe-talk-ready-toast-covers-text; the desktop strip follow is #1866.
