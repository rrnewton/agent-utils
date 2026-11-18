---
title: 'wrkslots-forked-log-journal-no-exit: a broken log plus any journal or seal has no sanctioned repair'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-18T00:05:24.000000000-05:00
updated_at: 2026-11-18T00:05:24.000000000-05:00
---

# Description

When the event log is unreplayable (a discontinuous revision) and any journal or validation-batch seal is also outstanding, no wrkslots verb can run: ordinary commands and `recover` replay the log and refuse (py/wrkslots/cli.py:10067, :90384, :90687); `recover-tail-event --yes` refuses on partial state (a dead seal bars it even with no journal) and on any outstanding journal (:15131 region); `quarantine-journal` covers scoped create journals only (:14816) and appends through a fully verifying event writer (:9812). Once any event lands after the discontinuity — the forking removal itself appends evidence and journal events (:62528, :60241) — it is no longer the tail, so tail recovery also refuses as broken-before-tail (:14989) and hand-moving the journal file no longer suffices. Found by exhaustive search in #1828 wrkslots-transition-model-check (report section 4.1, with minimal traces for the buried-fork+finish-journal and fork-at-tip+dead-seal shapes); the tests in py/wrkslots/tests/test_transition_model.py pin both shapes. Today this needs a #1807-class fork first; it is filed as defence in depth so a sanctioned repair exists before any second fork source (a new writer bug, an older binary beside a newer one) meets the ordinary coincidence of a stranded journal. Related: #1373 wrkslots-validate-before-append (recover-tail-event's origin).

# Design

Teach recover-tail-event a suffix set-aside: under the existing dry-run, operator-echoed sequence/sha, receipt, move-never-delete discipline, set aside the minimal suffix that makes the prefix replay, eligible when every outstanding journal's recorded operation provably postdates the discontinuity or belongs to the operation that wrote it (journal events after the fork point name their operation; a journal that predates it still bars set-aside). Independently, a proven-dead seal-only state (seal present, no paired finish) should be retirable by a path that does not replay the log, since the seal file plus the paired-finish check is self-describing. Keep both refusals fail-closed whenever provenance cannot be proven from the journal and prefix alone. Both editions where the verbs exist; no new persisted fields beyond the existing receipt shape extended to name every event and journal relied on.

# Acceptance Criteria

Deterministic tests transcribing the model traces: (a) buried discontinuity plus its own finish journal — sanctioned verbs alone return the registry to loadable, the removal then recoverable or safely refused with the journal intact; (b) discontinuity at the tip plus a dead seal with no paired finish — seal retired and tail set aside without hand file moves; (c) a journal predating the discontinuity still bars set-aside. Tests fail before (every verb refuses) and pass after; condition-based waits only.

# Notes

[watermelon-full-preview] Filed from #1828 wrkslots-transition-model-check at the coordinator's request; see ai_docs/reports/research/2026-11-18-wrkslots-model-check.md section 4.1 for the full refusal chain and state taxonomy.
