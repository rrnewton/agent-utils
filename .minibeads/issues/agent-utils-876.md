---
title: 'wrkslots-foreign-shard-write: a mutation never appends to another machine''s event log'
status: closed
priority: 0
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T09:21:58.468320581+00:00
updated_at: 2026-11-05T11:38:44.885509914+00:00
closed_at: 2026-11-05T11:38:44.885509773+00:00
---

# Description

P0. A wrkslots mutation on one machine appended an event to ANOTHER machine's event log, and that log is the source the other machine's table is rebuilt from.

**What happened on 2026-11-05:**
- A consumer project on the second host held a stale copied `EVENTS.<first-host>/` (11,103 files).
- At 03:49 ET, that project's cleanup worker on the second host went through the retirement path and appended `…011103.json` to it: kind `retirement-attempted`, `machine` = the first host, for a row that belongs to the first host.
- The first host's table for that row is rebuilt from that event log. So archiving the stale `ACTIVE.<first-host>.json` copy did not clear the row, and a foreign machine's history was rewritten from the wrong host.

**Likely mechanism:** `_event_directory(config, machine)` in `py/wrkslots/cli.py` accepts any machine name, so a caller that passes a row's `machine` writes into that machine's shard. Retirement and cleanup are the suspected callers, but every append site needs checking.

This is the write side of #873 wrkslots-local-shard-default (the read side). The two close together.

# Design

1. Every append to an event log (and every write to an `ACTIVE.<machine>` shard or journal) refuses when `<machine>` is not the local machine, as a hard error naming both machines: "refusing to append to EVENTS.<row-machine> from <local-machine>".
   - Put the check in the one function that resolves the event directory for writing, so no caller can bypass it.
   - Keep a read-only variant for `--all-machines` readers.
   - Audit every append site in both editions (Python and the Rust wrkslotsd), retirement and cleanup in particular, not only create/remove.
2. A local table rebuild never reads a foreign event log unless `--all-machines` is given. This matches #873's default for reads.
3. A command that meets a foreign row (for example retirement of a row whose `machine` is another host) reports it as belonging to that machine and skips it, rather than recording anything about it.

# Acceptance Criteria

- Test: with a foreign `EVENTS.<other>/` present (holding an active row with absent storage), a retirement attempt and a cleanup run refuse or skip that row, and no file appears in `EVENTS.<other>/`.
- Test: a local table rebuild ignores the foreign event log by default and includes it under `--all-machines`.
- Test: a direct append with a foreign machine name raises the named refusal, in both editions.
- Landed alone, with the SHA sent to the lead at once for the first host's deploy. #873 closes together with this issue.

# Notes

Reported by the lead on 2026-11-05 at 04:21 AM ET (P0). It blocks #873. Assigned to mu-validate, which takes it FIRST. The stale foreign event log copy on the second host is being archived by its operators.
