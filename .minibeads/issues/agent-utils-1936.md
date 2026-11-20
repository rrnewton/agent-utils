---
title: 'discuss-cache-84-85: screenshots run.sh Discuss cache trigger misses states 84 and 85'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] `vibe-talk/scripts/run.sh`'s Discuss-cache trigger
matches `--only` prefixes `73-|74-|75-|76-|78-` but not `84-`/`85-`, while
`screenshots.py` already classifies `84-discuss-timeline-layout` and
`85-discuss-terms` as Discuss scenes. A screenshots run naming only 84/85
therefore starts a cacheless throwaway server (`SHOTS_DISCUSS=0`, no
`[message_cache]` section) and fails, because Discuss reads the served
message cache (`#1855`).

# Design

- Add `84-*` and `85-*` to the `case` pattern that sets `SHOTS_DISCUSS=1`
  in `run.sh`, keeping the two prefix lists identical.

# Acceptance Criteria

- A `--screenshots --only 84-...` or `--only 85-...` run writes the
  `[message_cache]` (and `[summaries.llm]`) sections into its throwaway
  config, exactly as 73-78 runs do; non-Discuss states still get no cache.

# Notes

Filed and fixed by mu-flakes4. Browser verification is unavailable on
this host (missing system library), so the step is judged by the shell
trigger itself, not by a photographed run.

## Resolution

Landed on github/main as `07b2b854` (mu-flakes4, 2026-11-20).
