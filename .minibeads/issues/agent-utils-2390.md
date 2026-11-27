---
title: 'gate-admission-dir-out-of-tmp: gate.py admission state moves from the host temp dir to a per-user state dir, reading the old path during the switch so no ticket or running gate is lost or admitted twice'
status: open
priority: 2
issue_type: task
assignee: cl-tmpfix
labels:
- gate
- tmp
- validation
created_at: 2026-11-27T10:51:30-05:00
updated_at: 2026-11-27T10:51:30-05:00
---

# Description

`scripts/gate.py` keeps its admission state (queued tickets, running gates, holds and the files they are coordinated through) in `/tmp/dev-agent-utils-gate-admission` unless `$GATE_ADMISSION_DIR` names another directory. Agent work must not keep state in the host's `/tmp`. #2383 private-run-tmpdir moves every gate's scratch out of it, and the admission directory is the piece left behind.

The default path is also read outside gate.py:

- the wrkslots gate-run liveness check (Python CLI, `$GATE_ADMISSION_DIR` or the same default)
- `cross/loadgen.py`
- host operations scripts that read the queue and holds

local-ci-hub and the wrkslots sandbox read only an explicitly exported `$GATE_ADMISSION_DIR`. Readers pinned to older releases on a host keep the old default until they move.

# Design

- The new default is a per-user state directory, `~/.local/state/agent-utils/gate-admission/`. `$GATE_ADMISSION_DIR` keeps overriding it.
- The migration loses no ticket and admits nothing twice. While the switch runs, gate.py reads both the new and the old directory, and admits under one lock that covers both. A ticket or running gate recorded only in the old directory is honoured until its process is gone. New tickets go only to the new directory.
- The design note to the lead coordinator covers four points before any code:
  - the exact migration
  - old and new gate.py versions running at once on one host (an old gate.py queues only in the old directory, so admission stays correct only if the new one keeps honouring it, or the switch waits until no old gate.py is running)
  - readers of the default path
  - the date the old-path fallback is removed

# Acceptance Criteria

- A gate started on the new default keeps all of its admission state outside the host temp dir.
- On a host where tickets or running gates are recorded only in the old directory, a new gate.py neither loses them nor admits past them, and the two are never admitted at once.
- An old and a new gate.py running together on one host never exceed the slot cap or the exclusive class.
- Every in-repository reader of the default path follows the new default, keeping the old path as a fallback until the removal date in the design.
- The fallback removal date is recorded here.

# Notes

Filed 2026-11-27 10:51 AM ET at the lead coordinator's request. It follows #2383 private-run-tmpdir, which moves gate scratch out of the host temp dir, and comes after #2382 lands.
