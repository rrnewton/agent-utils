---
title: 'agentctl-revive-archived-journal-mismatch: revive --archived and starting new names failed rc 75, prepared stopped revive record does not match its journal'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-26T19:41:00-05:00
updated_at: 2026-11-26T19:41:00-05:00
---

# Description

UNVERIFIED since the report. Reported by a trusted consumer coordinator from 2026-11-20: `agentctl revive --archived` and starting new agent names both failed with rc 75 and the error "prepared stopped revive record does not match its journal". The failures blocked creating new reviewer seats at the time. The report has not been re-checked since 2026-11-20, so whether current agentctl still refuses this way — and on what record/journal shapes — is unknown; this issue is filed so the report has a number and the exact error text is not lost.

# Design

First reproduce or refute on the current build before changing anything:

- Recreate the record/journal pair the error names (a prepared stopped revive whose journal disagrees with its record), or produce one by the operations that plausibly write one (stop, archive, revive interrupted between prepare and journal), and run `revive --archived` plus a fresh `start` of a new name against it.
- If it reproduces, the fix follows the reader the error comes from: a mismatch between a prepared record and its journal should either be reconciled (the journal is the durable account of what actually happened; the prepared record is rebuilt from it) or refused with a remedy that unblocks unrelated work — starting a new, unrelated name must never be refused because one archived record is inconsistent. A refusal that names the remedy (which record, which field disagrees, and the command that repairs or retires it) is the minimum.
- If it no longer reproduces, say what was tried in the Resolution and close as not reproduced, naming the build tested.

# Acceptance Criteria

- The reproduction question is answered either way: the rc 75 mismatch is reproduced on a named current build with the record/journal shapes that trigger it, or the attempts made are recorded and the issue closes as not reproduced.
- If fixed: `revive --archived` on the reproduced pair either revives or refuses with a remedy naming the record and the disagreement, and starting an unrelated new name is never blocked by one inconsistent archived record (test).
- No journal or record is silently rewritten to make the mismatch disappear; any reconciliation is logged where the record's history is read.

# Notes

Filed 2026-11-26 at the coordinator's direction from the 2026-11-20 consumer report, status UNVERIFIED since. Possibly related reports from the same pass that were deliberately not filed at the same time: a Muse revive failing with "recorded environment names cannot be reconstructed" (unverified; may share the revive-record reading path, worth one look while reproducing this one).
