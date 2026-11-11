---
title: 'wrkslots-salvage-report-clarity: name the object behind each salvage count and bundle, and print each scan once'
status: open
priority: 3
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T14:41:03+00:00
updated_at: 2026-11-11T14:41:03+00:00
---

# Description

[opus 5.5] The first removal on a consumer host after #1280 wrkslots-kept-set-over-collection read as contradictory. For one checkout, the human output and receipt showed "stale fetched past prune expiry: 1 (not kept)" next to "bundled (origin not in salvage_push_remotes)". The receipt fields show both are true, about different objects:

- the bundle holds the checkout's salvage commit, archived locally because its origin is not a salvage push remote;
- the "not kept" count is a separate stale fetched-only tracking tip.

Nothing in the output says which object each statement covers.

In the same run, one nested checkout's count lines printed four times, once per scan (before and after the fence, and the final pass), with identical numbers.

# Design

- In the human output and receipt, name the object behind each salvage statement: the bundled salvage commit (with its disposition and reason), and the tips not kept (by category, with up to a few SHAs and refs inline and the rest in the receipt).
- Print the per-checkout count lines once, for the final pass. Earlier scans log only when their counts differ from the final ones.
- Python and Rust `remove_write.rs` together, keeping the differential exact; no new archive-record key.

# Acceptance Criteria

- A fixture with a locally archived salvage commit plus one stale tip past expiry prints one line saying the salvage commit is bundled, and one naming the tip as not kept.
- A removal that scans a checkout several times prints its counts once.

# Notes

Lead ruling 2026-11-11: one issue for both, taken by an idle lane, no new lane. Receipt fields confirmed by the consumer's coordinator. Not a data-loss bug: the dropped tip falls under #1280's fetched-only, past-expiry rule.
