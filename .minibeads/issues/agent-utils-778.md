---
title: 'agentctl-stop-kills-caller: agentctl stop signalled its own caller'
status: closed
priority: 1
assignee: cl-start@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T17:13:14.414803771+00:00
updated_at: 2026-11-02T17:37:52.817656898+00:00
closed_at: 2026-11-02T17:37:52.817656898+00:00
---

# Description

[opus 5.5] 2026-11-02 16:17:10Z on one host: a coordinator running as a Claude TUI, resumed from a headless coordinator's session (session lineage rebound from the headless record's session to its own), ran 'agentctl --registry <registry> stop coordinator --handover-to <itself>' with the services snapshot built at 2e454a81. The call returned 137 and the CALLER's own Claude process was then terminated (SIGTERM); the headless unit did stop. Likely cause: stop's process matching (native session id, cmdline, ancestor walk) resolved to the caller because it shares the session lineage with the record being stopped. Requirements: (1) confirm the matching path in the stop code; (2) stop refuses to signal its own caller or any ancestor of it, and refuses to signal a process that belongs to a different live record; (3) regression test: a fake record whose recorded session or cmdline matches the caller's own process tree; stop must not signal the caller; (4) land under the usual gate. Also decide and document whether stop of a headless record should signal anything beyond its systemd unit. (Gap 27 in a consumer team's restore plan.)

# Notes

## Resolution

[opus 5.5] Matching path confirmed. `stop` of a headless record signals nothing itself: it sends the wrapper `{"op": "stop"}` over its control socket. When the harness exits, the wrapper's `harness_exited` ends the harness's process group. It then called `session_processes(session)`, which scans all of `/proc` for any non-agentctl process whose argv names the session (`--resume S` / `--session-id S`), and SIGTERMs those, then SIGKILLs any left after 5 s. The caller's Claude Code had been resumed with `--resume S` on the headless session, so it counted as a "leftover writer" and was terminated. The `agentctl stop` beneath it died with it (137).

Fix:
- The wrapper ends only leftover writers in its own Linux session (`own_session_writers`). Its harness and anything the harness starts share that session. It never ends itself, any of its ancestors, or a process another live record of the registry pins as its harness (`harness_identity` / `custom_process_identity`, matched by start time). A writer elsewhere is left alone, and the next wrapper still refuses to start beside it, so a session never gets two writers. `status` now reports `writer_sweep: own-session`.
- `stop` asks the wrapper's status first. It refuses, changing nothing and sending no stop, when stopping would end its own process chain: a caller that is the wrapper or harness, or that runs inside the wrapper's session (an agent stopping itself), or, against a wrapper that does not report `own-session`, a caller that runs the session itself. The last case is this incident on a wrapper still running an older build. The refusal says how to proceed (end that process, or `kill -KILL` the old wrapper, which skips its sweep).
- Tests: a stopping wrapper spares a writer in another session and one pinned by another live record, but still ends its own leftover; `stop_would_end_caller` against a caller resumed on the session (fixed and older wrapper builds, the harness itself, the wrapper's own session); end to end, a stop whose wrapper would end the caller sends only `status`, never `stop`, and leaves the record running.

Decision on whether a headless stop should signal anything beyond its systemd unit: no. `stop` itself signals nothing, and it does not stop or disable the unit; it reports the unit to disable. The only signals come from the wrapper, to its own harness's process group and to leftover writers in its own Linux session. Documented in `stop --help`, the user guide and the takeover module.

Deploy note: the protection against an older wrapper's host-wide sweep works only from a `stop` built with this change. A wrapper still running an older build keeps its old sweep until it is restarted on the new build, and that restart itself runs the old sweep. So restart it only while no other process runs its session.

