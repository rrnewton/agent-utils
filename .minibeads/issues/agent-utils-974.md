---
title: 'muse-peer-approval-prompt: watch and send must recognise the Muse peer-approval dialog as waiting for a human'
status: closed
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-06T20:36:55.000000000+00:00
updated_at: 2026-11-07T17:34:49.000000000+00:00
closed_at: 2026-11-07T17:34:49.000000000+00:00
---

# Description

A Muse peer-approval dialog blocked a consumer's Muse lane for over 20 minutes on 2026-11-06. The dialog reads "Message from <peer> ..." with the choices "Allow once / Reject once / Allow for this session / Block". Enter did nothing until a Down key focused the menu. `agentctl watch-agents` reported the lane only as blocked/idle, not as waiting for a human, so nobody was alerted.

# Design

- Capture the dialog's screen (before and after the Down key) as a fixture.
- Readiness and watch classify it as a human-input prompt. The watcher alerts on the next quick check, through the same path as #675's blocked prompts, and names the dialog and its choices.
- `agentctl send` refuses to type into a pane showing this dialog, the way it refuses other approval prompts, and says why.
- Do not answer the dialog automatically. Whether to allow a peer is a human decision.

# Acceptance Criteria

- Fixture tests: watch reports the dialog as waiting for a human on the quick check; send refuses with a clear message; an ordinary idle Muse composer is unaffected.

# Notes

Requested 2026-11-06 by the lead coordinator. Assigned to mu-validate, the watch lane, after its P1 #961.

## Resolution

Fixed by 0fbbb641 "agentctl: read Muse's peer-approval dialog as waiting for a human" (mu-validate). A shared peer-dialog detector in agent_watch, the Muse TUI/status blocked classification, and captured focused and unfocused fixtures. Full selected gate on the change passed 41/41 plus 6/6 exclusive.
