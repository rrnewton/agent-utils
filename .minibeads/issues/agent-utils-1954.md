---
title: "gate-admission-ticket-partial-read: queue ticket is written non-atomically and readers can see a partial file"
status: open
priority: 2
issue_type: bug
labels:
- gate
- admission
- race
created_at: 2026-11-20T06:58:00-05:00
updated_at: 2026-11-20T06:58:00-05:00
---

# Description

[watermelon-full-preview] A gate queue ticket is created with `path.write_text(json.dumps(ticket))` directly at its final pathname (`scripts/gate.py:1838`; same pattern for deferred notes at line 1866 and deferred-state updates at line 1896). A reader can therefore observe the ticket between the file being created and its contents being flushed: an empty or partially written JSON document. gate.py's own `_read` tolerates this (`except Exception: return None`), but a strict reader that trusts the filename's presence crashes on the partial body.

Evidence from mu-recoverroll's path-selected gate of efd11ff2 (2026-11-20): exactly one step failed, `python.repository-infrastructure.test`, on `tests/test_gate_admission.py::test_the_phase_mark_requires_a_live_parent`. The test's `_wait_for_queue` (py/tests/test_gate_admission.py:225) waits only until the ticket filename exists, then the test calls `json.loads(ticket_path.read_text())` on `admission/queue/*.json` and got `json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)` — the decoder dump shows the body was the empty string while the writer was still writing under parallel gate load. The same test passes 5/5 alone under heavy-slot.sh (about 6.3 s each), so the failure is load/timing-dependent, not a deterministic error in what the ticket contains. Cause, in one line: the ticket is written non-atomically at its final pathname, and this reader does not tolerate a partial file.

# Design

Write admission state (queue tickets, deferred notes, run records) to a temporary file in the same directory and `os.replace()` it into place, so the final pathname only ever appears with complete contents; and/or make strict readers treat a parse failure as "not yet complete" and retry until a bounded condition-based deadline instead of crashing (owner rule 346: condition-based waits only, no wall-clock budgets). The atomic write fixes all readers, including future strict ones, without teaching each reader to retry; applying both sides is also acceptable. Whichever side the gate owner prefers, filename order stays enqueue order and no fairness/ordering behaviour changes.

# Acceptance Criteria

- A queue/deferred/run filename never exists with a partial body (temp file plus rename), proven by a regression test that reads the queue continuously while many tickets are enqueued.
- `test_the_phase_mark_requires_a_live_parent` passes under full parallel gate load, not only solo (5/5 solo passes are already demonstrated).
- `gate.py status` and `gate.py queue` output and ticket ordering are unchanged for well-formed state.

# Notes

Filed by mu-recoverroll from its own gate evidence (#1923 wrkslots-recover-late-census-rollback follow-up gate), at coordb's direction ("don't fix it in this lane"). Reserved number #1954 from coordb. Not a behaviour change to the census or any wrkslots path; the race is in gate admission state publication and one strict test reader.
