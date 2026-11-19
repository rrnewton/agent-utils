---
title: 'wrkslots-recover-late-census-rollback: a late census refusal in recover leaves the slot fenced and the seal blocking the host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- recover
- census
created_at: 2026-11-19T17:52:25-05:00
updated_at: 2026-11-19T17:52:25-05:00
---

# Description

[watermelon-full-preview] A consumer-reported defect at 74549a37. A validate-fresh run died mid-finish and left a validation-batch seal plus a finish journal, both with dead owners. `wrkslots recover --slot <that slot>` restored the seal ("durable evidence retained for finish retry"), then refused twice on the same-UID process census that runs just before deletion. That census gives up after 8 rounds, and any process exiting during a round fails it; on a busy many-core host (about 530 process starts a second) almost every round fails. While refused, the checkout stays fenced and the host-wide seal stays, which blocks every adopt on the host for hours. The earlier census already rolls the fence back on refusal. Ordinary validate removals hit the same late-census refusal. A usability trap compounds it: `--recovery-note` counts as a cleanup flag, so a plain recover with a note is misread as a new ownerless cleanup and refused. A pinned old reader (writer ceiling 2) must see no new persisted field, journal key or event kind; this fix adds none.

# Design

- When the late pre-deletion census refuses in recover, roll back exactly as the earlier census does: restore the checkout from the fence to its canonical path, and leave the seal and the finish journal in the state a later retry expects. The seal stays as strict as before. A refused recover leaves the host no more blocked than before the attempt: the slot is unfenced, but the batch's evidence is kept for a retry.
- Apply the same rollback in the remove finish path where the same gap exists. Do not redesign the census: the slice-based closure for validate-complete finishes and the absent-slot closure are a separate lane's work, and a census change that closed on "processes that touch the target" was reverted as unsafe.
- Accept `--recovery-note` on a journal-driven recover (it is not a cleanup flag there), or, if that is unsafe, make the refusal say exactly why and what to run. The note is not persisted by journal-driven recover, so no reader sees a new field.

# Acceptance Criteria

- A late-census refusal in recover (injected census outcome) leaves the checkout unfenced, the seal and finish journal retryable, and a retry succeeds once the census closes.
- One test with real short-lived processes churning makes a real 8-round census fail and shows the same rollback.
- A `--recovery-note` on a journal recover is accepted.
- At least one new test fails on the old code (74549a37); existing test doubles and assertions are unchanged.
- Focused wrkslots suites pass; the exact state left after each refusal is reported for cold review before landing.

# Notes

Priority high; consumer-reported. Related: the census redesign lane (slice-based closure for validate-complete finishes plus an absent-slot closure) is separate and under cold review; this issue is only the rollback and the usability fix. Cold review runs before landing because this touches remove/recover safety.
