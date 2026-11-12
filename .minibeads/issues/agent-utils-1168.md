---
title: 'read-toggle-two-way: make the read-messages toggle two-way, Show and Fold only'
status: open
priority: 1
issue_type: task
created_at: 2026-11-09T22:31:16.855597722+00:00
updated_at: 2026-11-09T22:31:16.855597722+00:00
---

# Description

Owner reply 280 (2026-11-09): "Actually related to this, I really
like the fold read mode... And I really don't like the hide read
mode. It's too confusing. Let's just disable the hide read mode for
now. You can leave it there in the code, but let's make the toggle
just a two-way toggle between show and fold".

The read-messages toggle today cycles three ways: Show read, Fold
read, Hide read. Hide leaves the code but becomes unreachable.

# Design

The toggle becomes two-way everywhere it can be reached: the bar's
button, its keyboard shortcut, the action (hamburger) menu wording
and the settings/help text that describes it. The cycle is Show to
Fold to Show. A stored preference of Hide loads as Fold, so a
reader who had Hide chosen lands on the nearest kept mode instead
of a mode no control can reach anymore. Hide's rendering path stays
in the code untouched, only unreached: nothing that still asks for
it by name keeps working by accident, which the tests pin.

Read-mode files are shared with other lanes; the exact files and
functions will be cleared with the coordinator before editing.

# Acceptance Criteria

- The toggle cycles Show, Fold, Show from the button and from its
  keyboard shortcut, and no menu, setting or help text offers Hide.
- A stored Hide preference loads as Fold.
- Existing Show and Fold behaviour is unchanged, including what
  each draws and what the keys do in each.
- Page tests cover the two-way cycle, the stored-Hide migration,
  and the absence of any UI path to Hide.

# Notes

Owner reply 280, 2026-11-09 5:26 PM ET. To be gated together with
#1166 switcher-frame-panel and #1167 jump-to-unread-chip under one
full gate after the coordinator's all-clear.

## Owner model

The to-do list is dropped with Hide read. Read and unread state are
both synced client <-> server; Slack is only a slow inbound source
(we do not go upstream for most message handling; we slowly get
messages FROM Slack), consistent with reply 414. The server /todo
endpoint stays for a follow-up; the page no longer reads it.
Built as two commits with #1399 toggle-labels-active-style for the
evening Discuss batch. [watermelon-full-preview]
