---
title: 'cross-test-tmp-cleanup-and-snapshot-prune: cross tests clean their temp trees and services snapshots can be pruned'
status: closed
priority: 1
issue_type: task
labels:
- agentctl
- test-hygiene
created_at: 2026-11-19T16:30:59-05:00
updated_at: 2026-11-20T02:44:28-05:00
closed_at: 2026-11-20T02:44:28-05:00
---

# Description

[watermelon-full-preview] Tests leave large temp trees behind:

- the agentctl cross differential's temp roots (tens of GiB in aggregate,
  most of it idle for more than a day);
- the Herdr-agent cross differential's temp roots (several GiB);
- the isolated agentctl registries the pytest shard, gate, validate and
  component-test helpers create under the system temp dir (several GiB);
- several GiB of agentctl services snapshots under the services state
  root, because services has no prune.

Existing idle trees are removed separately; this issue stops new ones
from leaking and gives services a safe prune.

# Design

- Every test or helper that creates one of those temp roots removes it
  on success, on failure and on interrupt (signal), through a
  try/finally or context manager plus a signal handler. A failing run's
  tree is kept only when AGENT_UTILS_KEEP_TEST_TMP=1 is set, and its
  path is printed when kept. Successful runs never keep the tree.
- `agentctl services prune` is a dedicated subcommand (not a step at
  the end of build/refresh): it keeps every snapshot referenced by a
  live unit, by a rollback record under the root's refreshes/, or by
  a turnover pin, plus the newest 3 unreferenced snapshots by default
  (--keep N), and deletes the rest. The default is a dry run listing
  what it would delete and why each kept snapshot is kept; --apply
  deletes. A dedicated command is auditable and safe to run in an
  emergency without building or restarting anything.

# Acceptance Criteria

- Cleanup tests show no temp tree left after a passing run and after
  a failing run, for both cross differentials and for the registry
  helpers; the keep opt-in keeps exactly the failing run's tree and
  prints its path.
- Prune tests use a fake state dir with referenced, unreferenced and
  live snapshots and never delete a referenced one; the dry run
  changes nothing and names a keep reason for every kept snapshot.
- All waits in the new tests are on conditions; no wall-clock budgets.

# Notes

Lane mu-crossclean/1917. Requested 2026-11-19. Prior work searched:
both cross differentials already remove their temp roots through a
shared signal-cleaned context manager, but without the keep opt-in;
the shard runner's basetemp already has a keep-on-failure opt-in of
its own. Neither covers failure retention or the registry helpers.
No services prune exists yet.

## Resolution

Landed d2c7da98 on github/main (feature 53f7462f, fixes d2c7da98;
gated df4a0856). Cross differentials and the shard/gate/validate/
component registries now clean up on success, failure and signal,
keeping a failing tree only under AGENT_UTILS_KEEP_TEST_TMP=1 with
its path printed. `agentctl services prune` (dry-run default) keeps
live, rollback-referenced, pin-protected and newest-3 snapshots.
Unverified: screenshots-desks waived (scene 59 pre-existing on the
gated base, fixed on main); no live-state prune was run.
