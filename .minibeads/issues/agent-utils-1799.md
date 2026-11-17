---
title: 'wrkslots-live-mutation-journal-reads-interrupted: a live remove journal reads as interrupted to a concurrent heartbeat'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T04:33:45-05:00
updated_at: 2026-11-17T04:33:45-05:00
---

# Description

[watermelon-full-preview] Consumer report (2026-11-17): during a LIVE `remove`, a concurrent `heartbeat` reported the remove's journal as "interrupted mutation ... run 'wrkslots recover'". A live mutation must read as BUSY (its recorded owner alive), never as interrupted: the interrupted wording invites a destructive recover against a running removal, the same defect class as #1736 wrkslots-live-seal-reported-as-interrupted for seals.

The busy classification exists: a finish journal whose recorded removing process (`_finish_journal_owner`, #1121) is live classifies live via `_journal_blocker`/`_assert_no_journal` and surfaces as `_JournalBusy` (exit 75), as fixed for create journals by #992. The question is which path the heartbeat took: `heartbeat` meets journals through `_assert_validate_cleanup_journals_disjoint_from_record` and the create-journal classification (`_classify_create_journals`), and a `--no-lock-budget` remove's finish journal may reach it through a reader that never checks the journal owner's liveness, or a finish journal shape (scoped versus global, no recorded owner generation) that classifies legacy/dead instead of live.

# Design

- Reproduce first: name the exact reader and journal shape that reported a live remove's journal as interrupted to a heartbeat (global versus scoped finish journal, recorded owner present or not, --no-lock-budget or ordinary).
- Make that reader use the same owner-liveness proof as `_journal_blocker`: live recorded remover -> busy (exit 75, holder pid named); proven dead -> interrupted with recover named; no recorded generation -> the legacy wording, unchanged.
- Coverage in both editions where the reader exists in both (classify/status paths are mirrored in wrkslotsd).

# Acceptance Criteria

- A deterministic test: with a live remove's finish journal outstanding (recorded remover a live process), a concurrent heartbeat reports busy naming the holder, never "interrupted" and never suggests recover.
- A test that the same journal with a proven-dead remover still reports interrupted with the recover command named.
- Python and Rust editions agree on the classification (differential or paired tests).

# Notes

Related: #992 (the create-journal busy classification), #1736 wrkslots-live-seal-reported-as-interrupted, #1795 wrkslots-no-lock-budget-remove-holds-locks-through-salvage (the remove shape in the report). Queued after #1795 in the mu-removelock lane.
