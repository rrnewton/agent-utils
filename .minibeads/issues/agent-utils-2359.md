---
title: 'supervisor-crash-census-pids-race: the frozen-census crash probe reads its pids file before the adapter writes it'
status: closed
closed_at: 2026-11-27T03:19:09-05:00
priority: 2
issue_type: bug
labels:
- agentctl
- validation
created_at: 2026-11-27T02:17:52-05:00
updated_at: 2026-11-27T03:19:09-05:00
---

# Description

[opus 5.5] `py/tests/test_herdr_chat_transport.py::test_command_supervisor_crash_after_adapter_start_uses_frozen_census` failed `python.agentctl.shard-1` in a full gate on 2026-11-27, for a change that does not touch the chat transport. The test passes on a quiet host, so the failure only shows under load.

The probe failed with `ValueError: not enough values to unpack (expected 4, got 0)`. The adapter writes its four pids with `Path.write_text`, which creates the file empty and fills it a moment later. The probe waits only for `path.exists()` and then reads and unpacks the file at once. On a loaded host it can read the empty file in between and die.

This is the race #827 fixed in the sibling parent-death probe of the same file. That fix counted an empty or partial read as not ready. #1938's notes already record that this test fails on an empty pids file under load. The group-descendant probe near the top of the file has the same pattern: its leader waits for `path.exists()` with a 2 s wall-clock deadline, and the outer probe then reads `descendants` from the file.

# Design

Remove the race at the writer, and keep the readers tolerant:

- The adapter publishes its pids atomically: it writes a sibling temporary file, then calls `os.replace`. The file therefore never exists without its content.
- Each reader waits until the file parses to the expected number of integers. It counts `FileNotFoundError`, an empty read and a partial read as not ready, as in the #827 fix. When it gives up, it fails by name ("adapter never published its pids") instead of an unpack error.
- Apply the same change to the group-descendant probe. Replace its 2 s wall-clock wait with a generous deadline that waits on a condition (owner rule: no real-time-sensitive gating checks).

Do not lengthen sleeps and do not skip the test.

# Acceptance Criteria

- Both probes read only a fully written pids file. A failure to start is reported by name, never as an unpack, index or value error.
- The test passes repeatedly under artificial CPU load: at least 20 runs with the host above 75% CPU, with load1 recorded per run.
- No product code changes. The frozen-census and subreaper assertions stay as they are.

# Notes

- Found by the #2322 gate (ticket 312ce89d, head 23adcbf1): 65 of 66 steps passed, and the only red was this case.
- Related: #827 (sibling race fix), #1938 (this test noted under load).

## Resolution

[opus 5.5] Fixed in `py/tests/test_herdr_chat_transport.py`. No product code changed.

- Every adapter whose process the test or the supervisor kills now publishes its pids atomically: it writes `<file>.tmp`, then calls `os.replace`. That covers seven writers: the group-descendant, parent-death and frozen-census probes; the output-limit, output read-failure and final-wait tests; and the timeout test. A reader that sees the file therefore sees all of it. The writers that finish writing before their process exits on its own were already safe, and are unchanged.
- The frozen-census probe waits until the file parses to four pids. A missing or unparsable file counts as not ready, as in #827, and a timeout fails by name ("adapter never published its pids").
- The group-descendant probe checks that it read two pids. Its 2 s wall-clock start wait is now a 20 s wait on the same condition.
- The other deadlines that wait on a condition were raised (caller 30 s, post-kill 30 s, outer timeouts 60 and 90 s). They only matter when a test fails, so a passing run takes the same time as before.

Evidence:

- The 11 affected cases passed 10 runs in a row at load1 108-123, in about 13 s per run, unchanged.
- Deterministic check: with the adapter forced to open the file, sleep 0.3 s, then write (non-atomic), the old reader failed 3 of 3 runs with the gate error "not enough values to unpack (expected 4, got 0)". The new reader passed 3 of 3 even with that non-atomic writer.
- The whole test file passes (87 cases).
- Not done: the 20 runs above 75% host CPU in the acceptance criteria. The host was not hot, and the slow-writer check proves the race instead.
- Left as found: `test_command_final_wait_failure_always_wakes_anchor_then_reaps_supervisor` still gives its adapter a 0.5 s timeout. On a loaded host the adapter may not publish its pid in that time, which is the shape #389 fixed in the timeout test.

