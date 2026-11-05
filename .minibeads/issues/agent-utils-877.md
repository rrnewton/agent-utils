---
title: 'wrkslots-reap-archived-schema: reap-stray-worktrees reads archived records with the archive schema'
status: closed
priority: 1
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T09:32:58.471043721+00:00
updated_at: 2026-11-05T13:03:20.510995042+00:00
closed_at: 2026-11-05T13:03:20.510994941+00:00
---

# Description

`wrkslots reap-stray-worktrees` refuses, even in its default report mode, on any project that has archive history. It validates ARCHIVED records with the active-row schema, so it fails with errors such as "missing coordinator_lease ... unknown archive_id ...".

- Reproduced on a harness with 11 archived records.
- On a consumer project, this blocks the supported cleanup of 116 nested scratch worktrees.

Found in a read-only registry triage on 2026-11-05.

# Design

- Validate archived records with the archive-record schema (the one `_archive_to_obj` writes), not the active-row schema. Active rows keep the strict validation.
- Report mode never refuses because of a record shape it merely has to read past. If an archived record is genuinely malformed, name it and continue the report. Only a mutating run refuses, and only when the malformed record bears on what it would remove.
- Same behaviour in both editions if the Rust edition implements the command.

# Acceptance Criteria

- A test project with several archived records (as written by the current archive path, and by an older one if formats differ) gets a full report from `reap-stray-worktrees`, and its apply path works.
- A deliberately malformed archived record is named in the report, and does not hide the rest.
- Rerun on the harness that reproduced it: the report completes.

# Notes

From the lead on 2026-11-05 (P1). Assigned to mu-validate after #876 and #873.
