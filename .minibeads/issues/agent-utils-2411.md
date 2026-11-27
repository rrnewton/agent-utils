---
title: 'vibe-talk test.unit and test.page-3 CPU-timeout hardening on hot hosts (600 s cpu budgets): condition waits and generous backstops per owner rule 346'
status: open
priority: 3
issue_type: task
labels:
- vibe-talk
- validation
- flake-triage
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-27T17:46:00-05:00
---

# Description

The 2026-11-27 frequent-red audit (P3) found two vibe-talk suites with untouched-head reds on a hot host:

- `vibe-talk.test.unit`: 13 heads (FLAKE 7, UNKNOWN 6). It is `cargo test --release` with `cpu_timeout: 600`.
- `vibe-talk.test.page-3`: 19 heads (FLAKE 9, UNKNOWN 10). It is the node page suite with `cpu_timeout: 600`.

No step log survives, so the CPU-timeout shape is likely but unproven for these heads.

# Design

- With kept logs (#2409), confirm which cases spend the CPU budget under load.
- Replace busy or polling waits with condition waits, and give each step a generous backstop that is not tied to load (owner rules 346 and 2026-11-21).
- A known-red entry only with proof that the step fails alone under load.

# Acceptance Criteria

- Both steps pass at load1 >= 237 for a whole run, and the evidence records load1.
- No wall-clock or tight CPU budget decides a verdict.

# Notes

Filed 2026-11-27 at the lead coordinator's request.
Source: the harness repository's `ai_docs/reports/measurements/2026-11-27-frequent-red-steps-audit.md`, item P3 and steps 11 and 19.
