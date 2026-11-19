---
title: 'wrkslots-old-seal-fallback-scan-rust-python-divergence: the Rust fallback scan accepts unrelated wrkslots processes as a seal operator'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T04:33:45-05:00
updated_at: 2026-11-19T00:00:00-05:00
closed_at: 2026-11-19T00:00:00-05:00
---

# Description

[watermelon-full-preview] Found 2026-11-17 during the #1744 combined-tree validate: `test_classify_live_seal_structured_blocking_reason_matches_python` diverged under parallel shards. For an old-format seal (no recorded operator in its extension slot), the Rust edition's fallback process scan (`other_live_wrkslots_processes` in `rs/wrkslotsd/src/ensure_states.rs`) accepted an unrelated parallel-shard wrkslots process as the seal's operator and reported the seal "in progress (pid 1069463)", while the Python edition (`_other_live_wrkslots_processes` in `py/wrkslots/cli.py`) judged the same seal interrupted. New-format seals judge the recorded operator identity and never scan; only the fallback for older seals can diverge.

The two scans are meant to be one rule: a wrkslots removal command for this project, acting on a sealed target. Somewhere they differ -- candidate causes are the same-project test (argv `--project-root` versus cwd, canonicalization of a deleted fixture root), the target match (`target=1` partition, path containment), or the wrkslots-command match itself.

# Design

- Reproduce hermetically first: a fake process table (a synthetic /proc tree, as `test_only_wrkslots_processes_of_this_project_can_hold_a_seal_open` already builds for Python) with an unrelated wrkslots process, a related one, and unreadable cmdlines; run both editions' scans over it. No dependence on whatever host processes happen to exist.
- Name the exact rule difference from that reproduction, then make the Rust scan match Python's rule (or the reverse, saying which is right and why: the scan is a fallback for seals written before the operator was recorded, so the stricter project/target proof is the safer default).
- Keep the fail-safe: an unreadable command line still counts as acting, and an unreadable process list still proves nothing.

# Acceptance Criteria

- A hermetic differential test over a fake process table: both editions agree on which processes hold a seal open (unrelated, related, unreadable-cmdline, other-project cases).
- The fix names the rule that differed and aligns it, with the choice justified in the commit.
- The live-host differential (`test_classify_live_seal_structured_blocking_reason_matches_python`) no longer depends on parallel-shard timing.

# Notes

Related: #1736 wrkslots-live-seal-reported-as-interrupted (the fallback scan this issue aligns), #1744 wrkslots-validate-batch-seal-left-after-start-refusal (where the divergence surfaced). Take after #1744 and #1735 in the mu-removelock lane.

## Resolution

[watermelon-full-preview] Landed on main as 526e5789: the Rust fallback scan now takes the FIRST --project-root in an argv, like Python's _wrkslots_project_of, so a wrapper argv embedding a whole wrkslots invocation as data no longer holds an old-format seal open in the Rust edition only. Identical hermetic fake-proc tables in both editions pin the rule.
