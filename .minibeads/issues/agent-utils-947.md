---
title: 'agentctl-revive-candidate-unpinnable: the session-to-pane resolver misses a resumed Muse pane'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T18:16:13.000000000+00:00
updated_at: 2026-11-06T18:16:13.000000000+00:00
---

# Description

During a reboot recovery, `agentctl retire-stale-registration --replacement-pane P --replacement-session S` refused with "expected exactly one live pane for session S, found 0". Pane P's foreground argv was literally `muse ... resume S`. The resolver that maps a native session to its live pane does not recognise a Muse started with `resume S`, so a correctly resumed lane cannot be pinned as the replacement.

# Design

- Find the matcher behind "expected exactly one live pane for session S" and add the Muse resume argv form, with its options in any order, beside the forms it already knows.
- Prefer the pane's foreground process argv when present; keep the "exactly one" rule.

# Acceptance Criteria

- A test with a pane whose foreground argv is `muse <options> resume S` resolves to that pane, and retire-stale-registration with that pane and session succeeds.
- Two panes resuming S are still refused as ambiguous.

# Notes

Reported 2026-11-06 by the lead coordinator during a reboot recovery. Related: #946, #948.
