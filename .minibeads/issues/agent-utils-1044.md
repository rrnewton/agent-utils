---
title: 'gate-priority-ticket: a coordinator-granted priority class in gate.py, bounded so it cannot starve the queue'
status: open
priority: 2
issue_type: feature
assignee: coordinator
created_at: 2026-11-08T03:25:00.000000000+00:00
updated_at: 2026-11-08T03:25:00.000000000+00:00
---

# Description

gate.py grants tickets FIFO, except that --rerun tickets go first. When one landing removes failures that cost everyone reruns (2026-11-07: the fix for the screenshot port collision, #1037, waited ~75 min in the queue while that collision kept failing other gates), a coordinator has no sanctioned way to move it forward. Renaming queue tickets by hand once froze every gate for 1.5 hours, so that is not an option.

# Design

- A priority ticket is requested only with a flag or env var that coordinators pass (for example `--priority REASON`, refused without a non-empty reason). Lanes do not self-assign it; a brief or the coordinator's own tooling passes it. The reason string goes into the ticket, history.jsonl and the ledger row.
- Grant order: --rerun tickets first, then priority tickets, then normal tickets, FIFO within each class.
- Starvation bound: at most one priority grant in a row while a normal ticket is waiting. After a priority grant, the next grant goes to the oldest normal ticket if one is waiting.
- `gate.py status` shows the class and reason of queued priority tickets; `gate.py stats` counts priority grants.

# Acceptance Criteria

- Tests: a priority ticket goes after a rerun and before older normal tickets; two priority tickets cannot both pass a waiting normal ticket; a priority request without a reason is refused; the reason appears in history.jsonl and the ledger.
- Help documents the flag, the bound and who may pass it.

# Notes

Requested by the lead on 2026-11-07; to be built after that night's landings, once the cap experiment summary is in.
