---
title: 'wrkslots-census-churn-tolerant: one process-use census semantics for both editions, tolerant of vanished PIDs, never blind to appearing ones'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T00:17:46.000000000+00:00
updated_at: 2026-11-09T01:03:25.000000000+00:00
assignee: cl-wrkid
---

# Description

The two editions prove "no process uses this slot path" differently, and the difference is fail-closed against fail-open.

- Python (`_capture_process_path_census`, and the remove-path scans behind `_assert_slot_unused`) is strict: three attempts in one fixed wall budget (60 s for the audit census, 30 s for a remove scan) that never resets, and an attempt is discarded when any live process's /proc entries change mid-walk. On a loaded host it refuses, and every row or removal that needed it fails closed: audit rows become UNKNOWN, removals refuse. Gate logs on both hosts show it under load averages of 100-235 ("relevant process evidence changed during three liveness attempts", "the /proc scan before the path fence exceeded its 30-second bound"); it is the shared cause of #1084 audit-census-reserve-flake and #1091 audit-lock-free-row-flake, and of the remove/reclaim/recover test flakes in the same shards. Real audits and removals on a busy host fail the same way.
- Rust (`capture_process_snapshot`, rs/wrkslotsd/src/remove_write.rs) lists /proc once, reads each listed PID, and silently skips entries that vanish. It never refuses for churn, but a process that appears after the listing is never seen: it can report a slot unused while a new process works in it. That is fail-open.

Until this lands, audit's DELETABLE is ADVISORY under load: nothing removes on audit alone (wrkslots remove re-checks slot use itself, below), and no automation may treat DELETABLE as proof that a slot is unused.

Scope (the lead, 2026-11-08): P1; the appearing-PID half first, in BOTH editions; one owner (cl-wrkid) for the Python census and the Rust snapshot. Covers every slot-use check, not only audit:

| Path | Python | Rust | What closes the appearing-PID window today |
|---|---|---|---|
| audit | `_capture_process_path_census` once, then `census.assert_slot_unused` per row | `capture_process_snapshot` once per audit | nothing: one listing, so a PID that appears after it is missed (DELETABLE is advisory) |
| remove / reclaim (ordinary) | `_assert_slot_unused`: lsof (one kernel-wide pass, same listing window) plus a direct /proc scan (`proc_root.iterdir()`), before the path fence; the fence renames the slot; the same check again after the fence (`_finish_remove_paths`, `after_fence=True`) | `assert_slot_unused` = one `capture_process_snapshot` before the fence (remove_write.rs ~1762) and one after it (~2094); no lsof | the fence plus the after-fence check: a process that appeared between the two checks is listed by the second; one that enters only by the old path cannot after the rename. Still open: a process that appears after the after-fence listing holding a descriptor it inherited from a listed process that exited (the vanished parent is tolerated, the appearing child is not listed) |
| remove-validate-batch, private cleanup recovery (`_recover_finish`), frozen terminal removal | private-cleanup fence (mode 0700, identity) FIRST, then `_capture_process_path_census` (shared census) and, for frozen removal, a fresh same-UID census | not ported | the private fence stops entry by path before the census lists; the inherited-descriptor case above stays open |

No path removes on a census alone without a fence, but every fenced path still rests on a single listing per check, so the fix applies to all of them: the shared census (audit and batch paths), the direct /proc scan in `_observe_slot_use_once`, the same-UID census, and Rust's `capture_process_snapshot` (audit and remove).

# Design

DECISION (the lead's direction, 2026-11-08): both editions adopt one semantics.

- A PID that vanishes mid-scan is tolerated: a process that no longer exists cannot be using the slot. Its partial evidence is dropped, not a reason to discard the attempt. Re-read only the processes whose entries vanished or changed, as #996 does for live entries that vanished, and keep evidence already verified for unchanged processes across attempts, as the user-systemd snapshot has done since 2026-10-19.
- A PID that appears mid-scan must still be seen: after reading the listed processes, list /proc again and read every PID not yet read, repeating until a listing adds nothing new (bounded; exhausting the bound is the only churn refusal left, and it names the PIDs still appearing).
- A PID whose identity changed (same number, new start time) counts as vanished plus appeared.
- The wall budget stays a budget for real work, not a casualty of retries: report which phase exhausted it.
- Rust gains the re-listing step (today it misses appearing PIDs); Python gains vanished-PID tolerance (today it discards the attempt). After this, the two editions return the same answer for the same host history.

# Acceptance Criteria

- The forced-churn differential added with #1084 (a vanishing PID and an appearing PID injected deterministically into both editions' census, no host load) passes; it is a strict xfail until this lands, so it flips when fixed.
- A vanished PID never causes a refusal in either edition; an appearing PID that uses the slot path is reported by both.
- Under gate-like load (load average above 100), repeated audit and remove runs no longer refuse for churn.
- Both editions; the pinned_process_evidence fixture from #1084 may then be narrowed or removed.

# Notes

Filed by cl-wrkid (devbig030) at the lead's request, 2026-11-08, from the #1084/#1091 diagnosis: a test plugin that makes the Python census refuse reproduces exactly the audit failures seen in gates, and the Rust audit answers where Python refuses because of the semantics above. Evidence: /tmp/dev-agent-utils-handoffs/wrkid-audit-family.md on devbig030; devbig014's ledger summary in /tmp/dev-agent-utils-handoffs/audit-parity-evidence-devbig014.md. The Rust half belongs to the Rust wrkslots owner via the lead.

Correction (cl-wrkid, same evening, from the forced-churn tests added with #1084): the Python census is blind to an appearing PID too, not only Rust. With a listed process killed and a new process started in the slot between the /proc listing and the reads, BOTH editions publish the row DELETABLE, while a process already in the slot before the audit blocks it in both. Python tolerated that particular vanish (the process was gone before its entries were read) and did not retry, so it never re-listed. The re-listing step in the Design is therefore needed in both editions. Strict xfails that flip when it lands: `test_audit_census_sees_appearing_pid_and_tolerates_vanished_pid` (Python) and `test_audit_forced_churn_matches_python` (both editions), in py/wrkslots/tests/test_audit_state_rust.py.

Evidence, a consumer host with 77 slots (a lane on the lead's host, 2026-11-08, via the lead): with that host's liveness probe fixed (dead 29 / alive 26 / unverifiable 22), `wrkslots audit` still showed DELETABLE 0 of 77 (54 BLOCKED / 21 UNKNOWN / 2 HELD). 47 of the reasons were audit's census failing under load at load averages of 100-200: an identity change (paraphrased by the reporter as "PID 802288 identity-changed"), then a "3.58 s" timeout that marked candidates UNKNOWN. Removal's under-lock census on the same host completed. That host runs the Python edition only (python3 -m wrkslots from its committed agent-utils, a0eeb67d).

Why the effective audit budget was 3.58 s and not the nominal 60 s (read from a0eeb67d, unchanged on main): it is the leftover of one shared budget, not per-slot slicing. `_capture_process_path_census` makes one 60 s budget (`_ABSENT_PROCESS_CENSUS_SECONDS`) for all three attempts and never resets it on retry, and every privileged find/grep it starts through `_run_root_owned_command` gets `timeout_seconds=budget.remaining_seconds()`, whose overrun refuses as "privileged read-only census exceeded {remaining}s". Two attempts discarded on changed evidence at that load spent about 56 s, so the third attempt's command started with about 3.6 s left. Removal completes because its under-lock check is the target-scoped same-UID census, which reads only this user's processes. Decision: no larger or load-scaled bound now; the bound is spent by repeated attempts, which the vanished-PID half removes (an identity change counts as vanish plus appear, so it no longer discards the attempt). Revisit the bound only if a single pass at load 200 is measured near 60 s; the refusal should then say how long the pass took.

Progress (cl-wrkid): the appearing-PID half is in the first #1102 landing, in both editions. Python: the shared census (`_census_appearing_generations`), the agent-removal batch scans before and after the path fence (`_ListingFollower`; a single `remove` goes through them), and the ordinary direct scan, which now also reads in full any generation started after lsof ran. Rust: `capture_process_snapshot` takes the target paths and follows the same closure (audit and remove). Closure, both editions: re-list /proc after the reads; a listing that adds no generation closes; otherwise two proven listings in a row (no new generation using a target not already seen in use) close; 64 listings at most. Tests: `test_audit_census_sees_appearing_pid_and_tolerates_vanished_pid`, `test_audit_forced_churn_matches_python`, `test_audit_heavy_churn_many_slots_matches_python` (77 slots, 60 vanishing, 60 unrelated births, one appearing), and `test_remove_forced_churn_refuses_like_python` (fires after the path fence; with re-listing disabled the Python edition removes a slot in use). Remaining: the vanished-PID half (Python still discards a whole attempt on changed evidence).

