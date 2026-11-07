---
title: 'wrkslots-remove-batch-salvage-budget: remove-agent-batch''s 60 s per-item budget runs out in salvage of slots with many nested submodules'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T19:14:11.000000000+00:00
updated_at: 2026-11-06T23:40:00.000000000+00:00
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

## Measurements appended by #943 gate-flake-wrkslots-proc-scan (mu-flakes)

Independent confirmation from the differential remove tests
(test_remove_matches_python and siblings, bounded contention):

- 2-core scope (CPUQuota=200% + stress --cpu 6): unchanged code 0/1,
  scan-CPU-bound code 1/2 — every failure the 60 s item budget.
- 8-core dagrun box (same stress): 19/20; run 20 died on the item budget
  at a trivial `git rev-parse --git-common-dir` after ~60 s of censuses
  and git work, on a slot with no nested-submodule salvage at all.

So the budget is also consumed by census/git pre-deletion work under
load, not only by salvage; the phase named in the refusal (a git
command) is where the clock ran out, not necessarily what spent it.
