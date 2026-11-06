---
title: 'wrkslots-shard-mount-namespace: wrkslots shards fail in full gates on mount namespace indeterminate for a live PID'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T18:04:37.530921232+00:00
updated_at: 2026-11-05T18:04:37.530921232+00:00
closed_at: 2026-11-06T19:18:43.000000000+00:00
---

# Description

[muse watermelon-full-preview] Filed from the #827 gate-flake readout on 2026-11-05.

The Python wrkslots shards are the top first-run gate failures of the last 24 h (ledger: 94 first-run gates, 57.4% pass): shard-0 10, shard-6 8, shard-1 6, shard-2 5, shard-7 4, shard-4 4, shard-5 3. One recurring mode, seen on shard-6 in two runs on different tests, is a refusal of the form "mount namespace is indeterminate for live PID N" for transient PIDs under load; the same cases pass solo. #875 wrkslots-census-exiting-pid (closed) added a bounded re-probe for a PID whose mount namespace is unreadable, yet the mode still recurs, so either the re-probe is not on the path these cases take or the PID stays live-but-unreadable past the bound.

Shard memory pressure is tracked separately in #883 wrkslots-shard-memory-and-heartbeat (mu-salvage); this issue is only the mount-namespace refusal mode. The two must not be conflated when a shard fails both ways.

# Design

- Reproduce one failing case under load and record whether the named PID is exiting, live in another namespace, or live with an unreadable mount namespace for the whole re-probe window.
- If the PID is genuinely still live and unreadable past the bound, the refusal is correct behaviour with an unactionable name: make the message say what was re-probed and for how long, and make the test fixture stop depending on transient host PIDs where it can.
- If the re-probe is not reached on this path, put it there, bounded as in #875.

# Acceptance Criteria

- A named failing case from a full gate is reproduced and its PID state classified with evidence.
- The fix (code or fixture) makes the classified case pass in a full gate; no sleep-only or skip-only change.
- The refusal text, when it still fires, names the re-probe window.

# Notes

Named in the #827 readout of 2026-11-05. Coordinator noted the mode on 2026-11-04 ("wrkslots shard-6 fails on 'mount namespace indeterminate for a live PID', two runs, different tests; passes solo") and asked for it to be filed if the ledger showed it recurring; it has.

Filed by mu-validate on 2026-11-05 from the #827 gate-flake readout (local number 901); renumbered to 906 on landing by the coordinator, since 901 was taken on main.

Moved from mu-validate to the new Muse ultra wrkslots lane mu-wrkslots on 2026-11-05 at 5:50 PM ET (the owner lifted the Muse lane cap; the lead asked for a dedicated wrkslots lane).

## Resolution

Landed in the mu-wrkslots stack at 84585684. Classification: the recurring case is a PID whose mount link is already released (exiting namespace init, now proven directly in the re-probe without the generation stat) or a genuinely live unreadable PID; the former is proven, the latter still refused with the re-probe window named (4 re-probes over 150 ms).
