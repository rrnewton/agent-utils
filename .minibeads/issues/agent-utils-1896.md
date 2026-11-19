---
title: 'wrkslots-batch-census-without-lsof: replace the remove-batch lsof census with the same-UID proc scan'
status: open
priority: 3
issue_type: task
labels:
- wrkslots
- census
created_at: 2026-11-18T22:00:00+00:00
updated_at: 2026-11-18T22:00:00+00:00
---

# Description

[watermelon-full-preview] Design question, no code yet. The agent remove batch's shared process census runs one `lsof +D` per batch (`_capture_agent_remove_batch_census`, bound 600 s). Measured 2026-11-18 on a loaded host: a single batch census holds ~1 core for ~126 s solo (lsof walks every process's descriptors against the target trees), and the retire test that exercises it takes 126-153 s on both sides of #1808. It is the single biggest CPU consumer in the wrkslots Python shards.

# Design

Replace the lsof batch census with the same-UID /proc scan the ordinary census already uses (`_scan_same_uid_generations`: fd/cwd/root/maps/mounts/argv/environ per generation, proven-unrelated excluded), extended to the batch's multiple targets with per-target attribution identical to lsof's rule (a record not below exactly one target is attributed to every target). Preserve the refusal semantics: indeterminate evidence refuses, positive matches accumulate, and the #1883 closure rules (death is proof; two consecutive rounds with no live not-proven generation) apply unchanged. The lsof path remains the fallback where /proc is unavailable or the scan cannot prove equivalence for a target shape lsof covers today (enumerate those shapes first; each needs a /proc equivalent or a documented refusal).

Acceptance: differential fixtures where lsof and the /proc scan judge the same synthetic process sets, with identical per-target verdicts; the retire batch tests pass with lsof absent from PATH.

# Notes

References #1883 wrkslots-census-never-closes-on-busy-host, whose closure model this reuses. Filed from the #1808 shard-CPU investigation: the census cost is pre-existing and host-scale, not a lock regression.
