---
title: 'gate-flake-muse14-resumed-drain: cross-environment Muse drain race under host load'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-07T03:30:00.000000000+00:00
updated_at: 2026-11-07T03:30:00.000000000+00:00
---

# Description

Flake: `cross.agentctl.differential-2` case `primary/muse14/resumed-drain` diverges under host load (observed load average 60-130 from another team's hermetic validate): the Rust drain blocks with "adopted Muse foreground harness process generation changed; nothing was typed" (75 pending) while the Python edition delivers, or the Python edition quarantines possibly_submitted (76). The same shard passes alone on the exact same tree (222/222 paired checks) and on pristine github/main. Seen by two independent lanes on 2026-11-06/07 (mu-revive validate18 on 98e574b0, mu-flakes on a tree with no agentctl changes, overlapping a full validate on this host). Suspected timing race in the adopted-Muse generation verification around drain. Either make the check load-tolerant/retrying or give the gate a solo rerun for this shard; never weaken the generation proof itself.
