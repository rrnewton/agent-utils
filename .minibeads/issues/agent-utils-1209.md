---
title: 'selection-survives-updates: an active selection and an unchanged row survive background updates'
status: open
priority: 0
issue_type: bug
assignee: mu-scroll
created_at: 2026-11-10T14:17:16.382151601+00:00
updated_at: 2026-11-10T14:17:16.384954467+00:00
---

# Description

Owner report (reply 296, 2026-11-10): "when I select something sometimes before I can press the copy button, my selection disappears... something is updating the UI in a way that destroys my selection".

Something replaces row DOM on a timer or background update while the row's content is unchanged - candidates: live-tail arrivals, reactions, relative-time ticks, read state, the unread recount, re-read rescans - and the replacement drops the user's text selection in the row.

Fix in place, or skip the re-render when the content is unchanged, so an active selection survives background updates and an unchanged row is never re-rendered. The scroll trace's new selection-loss and row-replacement entries (companion issue) name the path from the device; the audit lane's stability work is separate.

# Design

Find the replacer first with the trace's mutation entries (device) plus code reading of the timed updaters (live tail, reactions, relative-time ticks, read state, recount, rescans). Patch in place at the replacer, or make the redraw skip rows whose rendered content is unchanged (the row-drawing layer already compares in places - extend that comparison to the path that replaces).

Tests: an active text selection inside a row survives a background update landing; an unchanged row is never re-rendered (node identity kept) across the same update.

# Acceptance Criteria

- A text selection made in a message row is still the selection after a background update with unchanged content lands.
- An unchanged row keeps its DOM node across background updates (never re-rendered).
- Page/module tests pin both; the replacer is named in Notes with file and function.

# Notes

Companion to scroll-trace-layout-shift (this set filed 2026-11-10, owner replies 296/298). Diagnose from the trace's mutation/selection entries plus the timed updaters; mu-stable's automated stability lane is separate.
