---
name: pr-landing-planner
description: Conflict-graph + CI-aware, advisory pull-request landing planner. Given the open PRs targeting a base branch, it computes which truly conflict, classifies each red CI (real / flaky / stale-required-check / evaluate-once-race / runner-outage), computes freshness + hold reasons, partitions into parallel-safe groups, and recommends a per-PR action (land-now / rebase-then-land / refire-stale-gate / escalate-runner-outage / refire-ci / hold-fix / wait). Use when planning a landing wave over many open PRs, triaging which reds are real vs. benign, or wiring a tick reminder that surfaces ready-to-land PRs. Advisory only — it never merges or refires anything.
---

# pr-landing-planner

Fuses the PR conflict graph with LIVE CI health, freshness, and priority into one advisory landing
plan. The headline value is classifying WHY a PR is red — into real, flaky, stale-required-check,
evaluate-once-race, or runner-outage — so a lander does not treat benign gate noise as a failure. It
recommends per-PR actions and parallel-safe groups; it never arms, refires, or merges anything.

The CLI is the source of truth for usage — do not rely on this file for details. Run:

- `pr-landing-planner quickstart` — self-contained getting-started tour (add `--emit-demo` for a demo fixture).
- `pr-landing-planner --help` — commands and flags.
- `pr-landing-planner --userguide` — the full user guide (complete reference).

Canonical commands:

- `pr-landing-planner plan --repo OWNER/NAME --base integration --net-wrapper with-proxy --gh-cmd ./scripts/gh_human` — the full plan.
- `pr-landing-planner plan --fixture demo.yaml` — a plan from a fixture (no network).
- `pr-landing-planner plan --format actions ...` — tick-hub-integrable line output (Option B).
- `pr-landing-planner graph` / `status` — just the conflict graph / just per-PR CI health.
