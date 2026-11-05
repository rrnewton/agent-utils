---
title: 'queue-failed-digest: an agent is told about queued prompts that may not have reached it'
status: open
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-05T17:43:33.966192443+00:00
updated_at: 2026-11-05T17:43:33.966192443+00:00
---

# Description

When a queued prompt ends in an agent's `queue/failed/` (outcome `possibly_submitted` or failed), the recipient is never told. On 2026-11-05 the coordinator lost four lead requests this way during the #884 pane-lock window. Two of them went unfiled for six hours, and one was a P1 owner request. Nothing on the receiving side said a message might be missing.

# Design

- On the agent's next idle, deliver a one-line digest of failed or possibly_submitted entries it has not yet been told about: "may not have reached you: <id> <sender> <first line>". It goes out once per entry, and multiple entries are batched into one prompt.
- Record what was digested, so an entry is reported once and a later successful redelivery or explicit acknowledgement clears it.
- `agentctl requests` and `agentctl status NAME` list these entries (id, sender, time, outcome, first line), so an operator sees them without reading the queue directory.
- The digest is itself a normal delivery, and it must not land in failed/ silently. If it fails, it stays pending for the next idle.

# Acceptance Criteria

- Test: an entry in failed/ with possibly_submitted is digested on the next idle, exactly once, with the id, sender and first line.
- Test: three entries make one digest. A later successful redelivery of one of them clears it from the list.
- `agentctl requests` and status show the entries. Help and the userguide are updated.

# Notes

Requested by the lead on 2026-11-05 at 12:43 PM ET (P1, together with #904), after the coordinator's audit of failed/ found the lost messages (#901, #902 and #896's reply-154 spec were recovered from it). Composer-evidence false failures (a prompt typed but with no visible submission proof) are #896's job: positive evidence marks them delivered. Assigned to cl-start after #896.
