---
title: 'agentctl-profile-requires-bypass: every launch profile carries its harness permission-bypass flag'
status: open
priority: 1
issue_type: task
labels:
- agentctl
created_at: 2026-11-17T20:05:52-05:00
updated_at: 2026-11-17T20:05:52-05:00
---

# Description

[watermelon-full-preview] Owner rule (2026-11-17): every agent runs on
bypass permissions. A liaison launched from a profile without
`--dangerously-skip-permissions` hit a permission prompt and locked the
owner's channel for hours; the host's `profiles.json` was hand-fixed, but
nothing stops a recurrence. Every launch profile must carry its harness's
permission-bypass flag, and agentctl must refuse to load or start an agent
from a profile that lacks it, naming the profile, the harness, and the
exact flag to add. Accepted flags, each checked against the harness's
real `--help` on the filing host: claude `--dangerously-skip-permissions`
or `--permission-mode bypassPermissions`; codex
`--dangerously-bypass-approvals-and-sandbox` or `--yolo`; muse
`--approval-mode never`. agentcloudctl has no permission-bypass
equivalent (`--dangerously-skip-sandbox` is a sandbox switch, not
permissions): agentcloud profiles report "no bypass flag known" as a
warning, never a refusal, and no flag is invented.

# Design

One requirement helper per edition (`profiles.rs`, `profiles.py`):
given a harness and an argument vector, it answers whether a bypass flag
is present (either spelling, split or `=`-joined) and, when it is not,
the exact flag to add. Enforcement: profile validation refuses an
interactive claude/codex/muse profile whose argv lacks the flag, so
loading the config refuses; the interactive start path (profile and
ad-hoc `--harness` with `--harness-arg` alike) checks the effective
arguments; revive, restart, restore, and `--refresh-profile` relaunches
check the replayed or refreshed arguments the same way. Headless
Claude has no carve-out (coordb review): a headless Claude start,
`run-headless`, revive, or restart whose harness args lack
`--permission-mode bypassPermissions` (or
`--dangerously-skip-permissions`) refuses with the same message, while
`--permission-prompts host` keeps routing hook "ask" decisions to the
wrapper. Other headless profiles stay exempt at load (codex/agy argv is
empty by rule; the runners mediate or append the flag themselves). agentcloud
profiles gain a `permission_bypass_warning` ("no bypass flag known") in
the public profile listing, mirroring `blocking_question_warning`, and
the matrix gains a permission-bypass row in the same commit (harness
rule). Compatibility: registries whose profiles lack the flag (the
other long-lived registry on the filing host is the known case) will
refuse starts after they upgrade; that is intended, and the refusal
message says exactly what to add.

# Acceptance Criteria

- Per harness (claude, codex, muse), in both editions where the tool
  has both: a profile without the flag refuses to load with a message
  naming the profile, the harness, and the exact flag to add; a profile
  with either accepted spelling loads; ad-hoc starts without the flag
  refuse the same way, and with it pass the check.
- Revive, restart (including `--refresh-profile`), restore, and
  turnover relaunches from arguments lacking the flag refuse with the
  same message; the headless Claude wrapper's own permission handling
  is pinned by a test, and headless starts need no profile flag.
- An agentcloud profile loads and lists with the "no bypass flag known"
  warning and is never refused for the missing flag.
- `common/docs/agentctl/harness-support-matrix.md` records the new
  per-harness enforcement (and the agentcloud gap) in the same commit,
  and the agentctl profile documentation names the required flags.
- Deterministic regression tests fail before and pass after, in the
  Rust and Python editions; no wall-clock budgets.

# Notes

[watermelon-full-preview] mu-profilebypass. Number pre-assigned by
coordb. Prior work searched: `--dangerously-bypass-approvals-and-sandbox`
is already appended by the foreign (headless worker) runners for codex,
and `blocking_question_warning` (#1158) is the shape precedent for a
per-harness launch-argument rule surfaced in the public listing. The
diff goes to coordb before the gate.
