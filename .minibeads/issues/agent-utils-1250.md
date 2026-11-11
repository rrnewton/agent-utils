---
title: 'vibe-talk-timing-robust: the remaining timing-shaped browser checks leave gating until they judge behaviour only'
status: open
priority: 1
issue_type: task
assignee: mu-testaudit
labels:
- vibe-talk
- test-robustness
created_at: 2026-11-10T22:28:31+00:00
updated_at: 2026-11-10T22:28:31+00:00
---

# Description

[muse watermelon] Owner, reply 346 (2026-11-10): "Disable those tests and have an agent work to bring them back when they're robust. Each product lane must make its tests fast, high signal, and robust to running on a hot machine (>75% cpu on 316 HW threads). ... Anything else should be disabled until we can improve it." Harness rule: no real-time-sensitive tests in gating; every gating check is fast, high-signal and robust on a hot machine; anything else is disabled, with a tracking issue, until it is fixed.

The following `vibe-talk` browser checks judge wall-clock, frame, scroll or audio-clock timing, so a hot host can fail them while the page behaves. They are disabled from default validation selection, runnable by explicit opt-in, until each returns as a behaviour-only check in the pattern of #1248 fold-latency-behaviour-verdict (behaviour in gating, budgets as an opt-in benchmark):

- `vibe-talk.check.fold-steps-browser` (the reader's row held within a pixel at every frame)
- `vibe-talk.check.scroll-stability-browser`
- `vibe-talk.check.typing-scroll-browser`
- `vibe-talk.check.pace-stepper-browser` (P1: only coverage for the Pace voice control)
- `vibe-talk.check.read-aloud-browser` (P1: only coverage for tap-to-audible reading)
- `vibe-talk.check.wit-read-aloud-browser` (P1)
- `vibe-talk.check.local-read-aloud-browser` (P1)
- `vibe-talk.check.fallback-chain-browser` (P1)
- `vibe-talk.check.read-aloud-blocks-browser` (P1: block boundary gaps measured on the audio clock)
- `vibe-talk.check.read-aloud-views-browser` (P1)
- `vibe-talk.check.read-pause-reply-browser` (P1: only coverage for pause, reply and resume)

Evidence at filing: gate ledger, last 3 days to 2026-11-10, shows repeated gate failures of these steps on heads that later passed solo reruns (for example read-pause-reply 6 gate failures, read-aloud-blocks 3, fold-steps 3), and gate statistics for 2026-11-10 (first-try pass rate 23.7%, flake rate 56.8%, red-gate rate 75%) show timing-shaped steps are a leading red-gate cause. Each check's timing mechanism is recorded in the test-robustness audit (harness `ai_docs/2026-11-10-test-robustness-audit.md`).

Not in scope: `fold-latency-browser`, `keyboard-browser` and `keyboard-hides-bar-browser` stay enabled, handled by #1248 fold-latency-behaviour-verdict (proved 20 of 20 under a saturated CPU scope). The screenshot harness steps (`shots.screenshots-phones`, `shots.screenshots-desks`, `shots.screenshots-wide`) stay enabled while their fixed-port collision is fixed (bind an ephemeral port and pass it through); if that fix proves not small, they join this issue's disabled list instead.

# Design

- Disable by label in `vibe-talk/tests.dag.yaml` plus a `scripts/validate.py` filter: labelled steps are excluded from every default selection (path-selected and `--all` alike) and named in the plan output as skipped, with this issue named as the reason. An explicit opt-in (environment switch documented in `validate.py --help`) restores them unchanged, so each check stays runnable while disabled.
- Return path per check, P1 first: rewrite the verdict to behaviour only (audio starts, block order, pause holds and resume continues, stepper steps and clamps, folds draw, scroll holds the reader's row as drawn state), with waits on drawn-state events or settled geometry and no elapsed-time, frame-count or audio-clock-gap assertion deciding the verdict. Former budgets, where still wanted, become an opt-in benchmark outside gating, as in #1248 fold-latency-behaviour-verdict.
- Proof bar for re-enabling: 20 of 20 runs inside a saturated CPU-quota scope on this host class (316 hardware threads), plus a behaviour-broken variant still failing. Record the proof in Notes before removing a step's disabled label.

# Acceptance Criteria

- Each listed check is absent from default `validate.py --list` output (path-selected and `--all`), present with the opt-in set, and named in plan output as disabled under this issue.
- Each re-enabled check meets the proof bar above, P1 checks first (read-aloud family, pace-stepper, read-pause-reply).
- No other `vibe-talk` check changes verdict; `fold-latency-browser`, `keyboard-browser` and `keyboard-hides-bar-browser` are untouched.

# Notes

Filed by the test-robustness audit lane (mu-testaudit) at owner priority; disables land with `GATE_PRIORITY_REASON='owner reply 346 disable non-robust tests'`. Replacements for the P1 voice checks are handed to the vibe-talk lanes by the coordinator; they re-enable under this slug.

[muse watermelon-full-preview] Pace stepper proof (mu-timerobust, 2026-11-10): `vibe-talk.check.pace-stepper-browser` now waits on drawn state and server-side tickets (30 s failure bounds), its mid-read read runs on 30 s test blocks so the retake cannot be missed, and the retake requires a fresh ticket for the block the read started on (a plain count could pass on a later block asked ahead of time; the first solo desk run showed a 0.00 s "retake" from that). Ticket times are printed and the old 8 s bounds are `--benchmark` only. Proved by 20 of 20 consecutive runs inside one `systemd-run --user --scope` at `CPUQuota=400%`, `AllowedCPUs=200-203`, saturated by four spinners in the same scope (stopped with the scope after), three solo passes and one `--benchmark` pass under natural load (retakes 0.62-0.63 s printed), and a `page_sources.variant` page with `retakeReadAtPace();` removed failing with "a pace changed mid-read did not read the block again". The disable had not landed on `github/main` for this check when the replacement landed, so no label was removed; the coordinator was told so the disable skips this check.

[muse watermelon-full-preview] Read-aloud views proof (mu-timerobust, 2026-11-10): `vibe-talk.check.read-aloud-views-browser` now judges the strip by order (counts 1 then 2 then 3 present with strictly increasing positions) rather than by a 0.7 s minimum lit stay; the stay stays printed and enforces only under `--benchmark`. Tight 5 s / 8 s state waits became `STATE_S` failure bounds, and the silent-read stop check waits for the drawn reading/pending state to empty instead of a fixed 500 ms. Proved by 20 of 20 consecutive passes inside one `systemd-run --user --scope scope` (`CPUQuota=400%`, `AllowedCPUs=208-211`, four spinners in-scope, trap-killed on exit), natural-load solos, one `--benchmark` pass (stay 1.53 s; proof series printed 0.99-1.02 s), and a mutation making the fake agent voice always silent failing with "the read never reached its third block" while the genuine silence control still stops the silent message unarchived. The disable had not landed for this check either, so no label was removed; the coordinator was told the same for pace landings before the disable.
