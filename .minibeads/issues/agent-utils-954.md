---
title: 'wrkslots-remove-batch-salvage-budget: remove-agent-batch''s 60 s per-item budget runs out in salvage of slots with many nested submodules'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T19:14:11.000000000+00:00
updated_at: 2026-11-06T19:14:11.000000000+00:00
---

# Description

On a consumer host at load about 50, `remove-agent-batch` refused all 7 dead slots with rc=3. The 60 s per-item lock budget was used up by salvage of slots that carry several nested submodule checkouts (five, up to three levels deep). One example stopped at `git remote get-url --push` inside a fenced nested checkout. The workaround is a single `remove --no-lock-budget` per slot.

# Design

- Scale the per-item budget with the number of checkouts salvage has to visit, or leave salvage I/O done before the lock out of the budget entirely, and keep the budget for work done under the lock.
- Report which phase used the budget when an item is refused.

# Acceptance Criteria

- A test with a slot carrying several nested submodule checkouts and slow git commands: batch removal succeeds within a scaled budget, or salvage time does not count.
- A refusal names the phase that ran out of budget.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer registry. Part 1 of 3 with #955 and #956. Queued for mu-wrkslots after #950.
