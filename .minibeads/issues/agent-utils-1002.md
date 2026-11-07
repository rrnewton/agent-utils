---
title: 'gate-flake-muse14-resumed-drain: cross-environment Muse drain race under host load'
status: closed
priority: 2
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T03:30:00.000000000+00:00
updated_at: 2026-11-07T16:53:22.000000000+00:00
closed_at: 2026-11-07T16:53:22.000000000+00:00
---

# Description

Flake: `cross.agentctl.differential-2` case `primary/muse14/resumed-drain` diverges under host load (observed load average 60-130 from another team's hermetic validate): the Rust drain blocks with "adopted Muse foreground harness process generation changed; nothing was typed" (75 pending) while the Python edition delivers, or the Python edition quarantines possibly_submitted (76). The same shard passes alone on the exact same tree (222/222 paired checks) and on pristine github/main. Seen by two independent lanes on 2026-11-06/07 (mu-revive validate18 on 98e574b0, mu-flakes on a tree with no agentctl changes, overlapping a full validate on this host). Suspected timing race in the adopted-Muse generation verification around drain. Either make the check load-tolerant/retrying or give the gate a solo rerun for this shard; never weaken the generation proof itself.

# Notes

## Resolution

[opus 5.5 cl-sync] Not a race in the generation check: the check was right. The cross-harness fake
Herdr started each stand-in harness process (the pinned Muse foreground) to exit after 60 s. Under load
the muse14 case's ~20 serial CLI calls take longer than 60 s, so the stand-in exited before the
drain, and agentctl correctly reported that the pinned process generation had changed.

Fix (cross/ only; the generation proof is unchanged):

- `cross/herdr_agent_differential.py`: a stand-in now lives as long as its case directory (it polls
  every second and exits once the directory is removed, or when the case retires it), never on a
  timer.
- `cross/agentctl_differential.py`: before the drain, `primary/muse14/fixture-alive-before-drain`
  requires both editions' stand-ins to be alive, so a dead fixture is reported as a fixture error
  naming the case, not as an edition divergence.

Evidence: only the muse14 group, each run under 6 busy-loop burners pinned with it to 4 CPUs,
inside solo gate tickets.

- Before (main's cross/): 5 of 5 runs FAILED (62 s each, load average 25-43). In every run both editions' stand-ins were already dead at 60.2-61.0 s, just before the drain, and the drain diverged (staged/held, resumed-drain, delivered-once-in-order).
- After (this fix): 25 of 25 runs passed (74-88 s each, load average 29-45). Both stand-ins were alive before every drain; 22 of the 25 drains came after 60.6 s, the point where every before-run had already lost them.
- Unloaded, the drain comes ~33 s after launch, which is why the 60 s timer only bit under load.
