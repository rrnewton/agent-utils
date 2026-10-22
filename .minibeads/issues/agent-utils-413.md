---
title: 'wrkslots-agent-name-validation: refuse agent names the liveness probe cannot judge'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-10-22T02:50:57.751886867+00:00
updated_at: 2026-10-22T05:48:21.554705788+00:00
closed_at: 2026-10-22T05:48:21.554705708+00:00
---

# Description

[muse watermelon] wrkslots create accepted an agent name with capital letters. The liveness probe rejects that name with a usage error, so removing the slot later fails with 'registered liveness authority is unverifiable', and the session tooling only accepts lowercase names. Make create, adopt and transfer-owner refuse agent names the session tooling cannot use, with the same rule it applies (1-32 lowercase letters, digits and hyphens). Also give a coordinator a documented, safe way to remove a slot that no agent was ever started in (never adopted, no owner process) when the liveness probe cannot judge its recorded name.

# Acceptance Criteria

create/adopt/transfer-owner refuse names outside 1-32 lowercase letters/digits/hyphens with a clear error; a documented removal path exists for a never-adopted ownerless slot whose recorded name the probe cannot judge, and refuses slots with an owner, adoption history, or local work; tests cover both.
