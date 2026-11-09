---
title: 'wrkslots-census-churn-tolerant: one process-use census semantics for both editions, tolerant of vanished PIDs, never blind to appearing ones'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T00:17:46.000000000+00:00
updated_at: 2026-11-09T15:00:00.000000000+00:00
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

Part 2 scope, from the daytime load reproduction (cl-wrkid, 2026-11-09 04:05-04:13 ET, at the stack tip on dab2a70c, after part 1): no census refused, but the near-misses have a clear cause. Method: the Python all-process census over a scratch slot, with every `_ProcessEvidenceChanged` logged, in three load profiles:
- A: 16 process-churn loops, load ~40: 20/20 ok, 4.5 s typical;
- B: 64 churn loops plus 160 CPU hogs, load ~240: 15/15 ok, 7-8 s typical;
- C: 200 churn loops plus 40 CPU hogs, load ~265: 15/15 ok, ~6 s.

"PID exited during mountinfo census" was the most frequent event, about 350, and costs nothing: part 1 already absorbs it. Every slow run, 13.8-15.9 s with one or two of the three attempts discarded, hit one of two whole-attempt discards:
1. "PID N exited as thread-group leader while its threads still run" (`_assert_leader_released_its_threads`);
2. "privileged find census produced diagnostics: find: /proc/X/fd/N": an fd that vanished while find read it, for a process still alive (`_LiveEntriesVanished` from `_allow_only_vanished_process_diagnostics`).

Two of those in one census refuse it, which is what gate shards with many threaded rustc and pytest processes hit. Logs: /tmp/cl-wrkid-1102/phase{A,B,C}.log.

Safety points for part 2 (the lead, 2026-11-09):
1. A thread-group leader that exited while its threads still run is a LIVE process for slot-use purposes. Its threads keep cwd, root, open fds and mappings, and /proc/PID/task/*/ still shows them. "Follow it" means reading the surviving threads' cwd, root, fds and mappings from /proc/PID/task/TID/..., never treating the PID as vanished. Required test: a leader exits via pthread_exit while a thread holds a cwd in the slot; that must BLOCK.
2. An fd vanishing mid-find: re-read only that PID's fds, with a bound. An fd that is gone counts as gone, not as a failed census.

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

Verbatim from the consumer host with 77 slots (Python edition at a0eeb67d), run A, `audit --format json`, DELETABLE 0 of 78, carried by all 21 liveness-dead UNKNOWN slots: `process/path census failed: PID 894871 identity changed after privileged find matched a selected path; the selected path may still be in use`. Run B, minutes later, had no occurrence (intermittent under load), and neither run reproduced the earlier "3.58 s" text. So the identity change in the field is the post-match recheck in the privileged find matcher, not only a /proc listing.

Vanished-PID half, part 1 (cl-wrkid): a matched holder whose generation is gone at its recheck is followed, not refused, in every matcher (privileged and same-UID find, maps, mount, and the same-UID direct census), whenever the census lists /proc again afterwards (the all-process and same-UID censuses; `_FOLLOWED_VANISHED_HOLDERS`). The gone generation's match is dropped; its replacement, or a child it handed a reference to before exiting, is a generation the next listing reads. Callers that do not list again keep the refusal. Test: `test_audit_follows_a_matched_holder_replaced_before_its_recheck` (a seam replaces the holder exactly at the recheck; with the old refusal the row is UNKNOWN, now it is BLOCKED by the replacement), both editions. Remaining, part 2: an attempt is still discarded as a whole on other changed evidence (`_ProcessEvidenceChanged` from a reader), which is what spends the shared budget; keeping verified evidence across attempts waits for the daytime load reproduction to show which refusals dominate.

Vanished-PID half, part 2 (cl-wrkid, 2026-11-09), per the lead's two safety points in the Design:
1. A thread-group leader that exits while its threads still run is followed, never treated as vanished. Python: inside the censuses that list /proc again (all-process and same-UID), `_assert_leader_released_its_threads` records the leader in `_FOLLOWED_VANISHED_HOLDERS` instead of discarding the attempt; the next listing observes each surviving thread in its place and reads its cwd, root, descriptors and mappings at /proc/PID/task/TID. A round that followed anything cannot close the census, though it does not reopen it either (all-process), and a followed leader is not proven (same-UID), so that listing always comes; a surviving thread's mount namespace is read again, in case the exited leader was its only representative. Callers that do not list again still discard. Rust: `capture_process_snapshot` had no thread handling at all: a zombie leader's links read as no evidence, so a slot held only by a surviving thread was published DELETABLE (fail-open, confirmed by the new differential against the previous source). It now reads each surviving thread (`surviving_threads`), both when a listing finds the leader already exited and when it exits while being read, and names it as `thread TID of process PID (whose main thread has exited)`, as Python does.
2. A descriptor that keeps vanishing for a live process through the bounded re-reads (`_LIVE_ENTRY_REPROBES = 2`) is gone: `_privileged_census_output` returns instead of discarding the attempt, keeping the last re-read's records. Only descriptor entries (/proc/PID/fd/N); a cwd, root, exe or maps entry that keeps vanishing still discards. Residual, recorded in the docstring: a reference moved by dup2 to a descriptor the last walk had already passed, in the instant before it read the closed one.

Tests: `test_leaders_that_exit_before_their_readers_are_followed_and_block` (three children, each the only user of its own slot through a surviving thread's cwd, a leader exiting before each check; privileged and same-UID lanes; on the previous census both lanes refused "relevant process evidence changed during three liveness attempts", now all three slots BLOCK and the first is named by its thread), `test_audit_follows_a_leader_that_exits_after_the_listing_like_python` (both editions through the churn seam; previous Rust: not BLOCKED), `test_find_census_counts_a_descriptor_that_keeps_vanishing_as_gone`, `test_find_census_still_refuses_other_entries_that_keep_vanishing`; the two zombie-leader "discards_the_attempt" tests are renamed `_is_followed`.

Still open after part 2 (both small items done in part 3, below):
- the same-UID census's own diagnostics (`_same_uid_indeterminate_processes`) still discard its scan for a vanished descriptor of a live process; it keeps positive matches and allows 3 discards, and the load reproduction never hit it;
- a surviving thread that executes a new program (taking its leader's PID) still discards the attempt: its new program has a generation the census already read, so re-listing alone would not read it;
- question for the owner (not probed: the root probe was declined at night): what root's find reports for a zombie leader's /proc/PID/fd. As this user it is "Permission denied", which the privileged census refuses outright, not as churn. A leader exiting between the census's check of it and find's walk would hit that if root sees the same; the tests here drive the exit before the check;
- acceptance criterion 3 (repeated audits and removals at load above 100 no longer refuse for churn) needs the load reproduction rerun on this code.

Part 3 (cl-wrkid, 2026-11-09), the two small items:
- The remove path's same-UID census reads through the same bounded re-read as the privileged one (`_census_output_rereading_live_entries`, shared by both): entries that vanish from a live process mid-find or mid-grep are read again for just that process, at most `_LIVE_ENTRY_REPROBES` times; a descriptor still gone is gone, any other entry still discards the scan. Test: `test_same_uid_census_rereads_a_live_process_whose_entry_vanished` (fd and cwd); the missing-path control tests now expect the same re-reads in both lanes.
- A surviving thread that executes a new program is followed inside the re-listing censuses: `_assert_thread_kept_its_identity` records the leader's PID (so the round cannot close) and the leader's exact generation in `_REREAD_GENERATIONS`, which the census forgets having read, so the next listing reads the new program even if that generation was read before the leader exited. Rust did not need the forgetting: its read of the leader while it still ran showed what the group then shared. It did have a narrower gap: a thread listed under a zombie leader and then read after its execve showed nothing, and the revived leader was read only by a next listing that a closing round could skip. `admit_threads` now reads a leader that runs again in the same listing. Tests: `test_threads_that_execute_before_their_readers_are_followed_and_block` (three children, both Python lanes; on the previous census both refused "changed during three liveness attempts"); `test_a_thread_that_executes_after_the_snapshot_is_followed` (renamed). The Rust race needs an execve between the task listing and the thread read, which no seam reaches; it is covered by reading, not by a test.

Remaining: acceptance criterion 3 (the load rerun, when the gate queue is empty), and the owner question about root's view of a zombie leader's fd directory.

## Resolution

Delivered in both editions (cl-wrkid, 2026-11-08/09):
- appearing-PID half: 89b240dc (re-listing closure in the shared census, the agent-batch scans and the direct scan; Rust `capture_process_snapshot`);
- vanished-PID half, part 1: 8948e67e (a matched holder gone at its recheck is followed, not refused);
- part 2: f0728d34 (a thread-group leader that exits under running threads is followed through /proc/PID/task/TID in both editions; the Rust snapshot had no thread handling at all, a fail-open; a descriptor still gone after the bounded re-reads counts as gone);
- part 3: 30a28e0b (the same-UID census gets the same bounded re-read; a surviving thread's execve is followed).

Load rerun after parts 2 and 3 (2026-11-09 09:43-09:54 ET, gate queue empty, other lanes holding): the morning's profile B (64 churn loops plus 160 CPU hogs) over the host's own load, measured 268-291. The all-process census ran 15/15 ok in 8.0-10.7 s, with zero thread-group-leader discards and one vanished descriptor absorbed by the re-read. In the morning, at load ~240 and before part 2, runs were 7-8 s with 13.8-15.9 s outliers, each from discarded attempts. `test_audit_heavy_churn_many_slots_matches_python` passed 8/8 under that load and 20/20 across the day on part-3 code (load 45-291). Logs: /tmp/cl-wrkid-1102r/ on devbig030.

Unverified:
- Acceptance criterion 3 was measured for audit, not for remove under load.
- The heavy-churn differential failed twice that day with every Python row UNKNOWN (the audit's one census refused): once on a tree with only part 2, once in another lane's gate on a367e847 (part 2 in its final form). Both were before part 3, and the refusal reason was never captured; part 3 does not obviously address such a refusal, so its cause is unexplained rather than known fixed. The test's assertion message now carries the first UNKNOWN row's reasons, so a recurrence names its cause in the gate log.

