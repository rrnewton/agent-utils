---
title: 'supervisor-crash-census-pids-race: the frozen-census crash probe reads its pids file before the adapter writes it'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- validation
created_at: 2026-11-27T02:17:52-05:00
updated_at: 2026-11-27T02:17:52-05:00
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
