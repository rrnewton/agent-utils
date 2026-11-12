---
title: 'mq-trust-base-across-runs: count a queue landing from an earlier run as base evidence'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-12T16:56:28.986214189+00:00
updated_at: 2026-11-12T16:56:28.986214189+00:00
---

# Description

With trust_base on, a red singleton may be blamed without probing the base when the base is a validated queue landing. But base evidence is per run: the base counts as a queue landing only if this run landed it, or it is a focused landing still owed a complete run. A base the queue itself landed in an earlier run after a complete validation is therefore treated as reached outside a validated landing, even though the landed change's own persisted record says it landed there after a complete validation. Every new run starts on such a base, so every red singleton probes its base. All three base probes in the drain evidence came back inconclusive (every selected test passed, exit 4), so trust_base never applied and each run stopped (see #1379 mq-unknown-probe-singleton). Prior work: #1187 mq-singleton-eject (closed) trusts only bases the queue landed after validating; this extends that same trust across runs instead of re-deriving it per run.

# Design

In rs/merge-queue/src/run.rs (Runner::base_evidence): count the candidate's parent as a validated queue landing also when the queue's persisted records say it landed there: any persisted item (State::items) with status landed whose landed commit equals the parent. Focused landings are already covered by the obligations path (every focused landing records an obligation, and a discharged obligation means a green complete run later covered that landing), so a landed record with no open focused obligation is a complete landing; keep the current-run and obligations checks and add the persisted check beside them. The recorded green at the base tree stays per run as today (only the landing fact crosses runs). No config change and no new persisted field: the landed record written at landing already carries the commit, the batch and the per-commit map.

# Acceptance Criteria

A singleton on a base that a previous run landed completely (a persisted landed record names the base commit) is ejected on its deterministic red without a base probe, as with a base landed in the same run. A base with only a persisted focused landing still owed its complete run keeps today's obligations behaviour. A base no record names (reached outside the queue) is still probed before any singleton blame, per #1187 mq-singleton-eject. cargo test -p merge-queue (all targets) passes, including the new cases.

# Notes

Evidence: a 2026-11-12 drain report relayed to this lane: the probed base was the previous run's complete landing (its landed change's record names the landing commit, the batch and the per-commit map), landed about half an hour before the run that probed it. The other two base probes since the day before were the same shape. Related: #1379 mq-unknown-probe-singleton (what an inconclusive probe does until the base is trusted), #1377 mq-validator-purpose-env (the adapter-side exit that will make such probes conclusive).
