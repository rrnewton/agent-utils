---
title: 'github-triage-pass: triage the 19 open GitHub issues and keep triaging'
status: open
priority: 1
issue_type: task
labels:
- tasks
- github
depends_on:
  agent-utils-631: parent-child
created_at: 2026-10-28T13:57:20.294215761+00:00
updated_at: 2026-10-28T14:20:48.533327077+00:00
---

# Description

[opus 5.5] Part of #631 task-tracker-adapters (near-term fix 3). GitHub is the report tier: serious bugs and outside reports. GitHub and minibeads numbers overlap (both run into the hundreds), so this issue writes GitHub issues as 'GitHub #N' and local ones as '#N'.

On 2026-10-28 GitHub had 19 open issues: 5 from August (GitHub #94, #95, #113, #123, #134), 7 from 2026-10-05..11 (GitHub #230, #231, #234, #237, #238, #239, #240), GitHub #286, and 6 filed 2026-10-27/28 by agents of other projects (GitHub #289-#294). Only one issue (GitHub #287) had been closed since 2026-10-14.

Wanted, one pass now:
1. For each open issue decide: fixed (close with a note naming the commit), not planned (close with the reason), or real. Label outside reports 'external'.
2. Mirror each real one into the local tracker for dispatch (external_ref gh-<n>; mb github link can keep them in step) and comment on the GitHub issue with the local issue id, so the reporter sees it was taken.
3. Merge duplicates: GitHub #292 (delivery double-check marks) overlaps #621 send-double-check (being filed by another lane; not on main when this was written).
4. Severity: the fuse2fs O_TRUNC report (GitHub #290, corrupts git files) goes first.
Mind the shared GitHub budget: read through gh-paced, batch writes, and settle which account posts before writing.

Done when every open GitHub issue is either closed or carries the 'triaged' label and a local-tracker mirror.
