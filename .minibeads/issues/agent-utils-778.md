---
title: 'agentctl-stop-kills-caller: agentctl stop signalled its own caller'
status: open
priority: 1
assignee: cl-start@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T17:13:14.414803771+00:00
updated_at: 2026-11-02T17:13:14.414803771+00:00
---

# Description

[opus 5.5] 2026-11-02 16:17:10Z on one host: a coordinator running as a Claude TUI, resumed from a headless coordinator's session (session lineage rebound from the headless record's session to its own), ran 'agentctl --registry <registry> stop coordinator --handover-to <itself>' with the services snapshot built at 2e454a81. The call returned 137 and the CALLER's own Claude process was then terminated (SIGTERM); the headless unit did stop. Likely cause: stop's process matching (native session id, cmdline, ancestor walk) resolved to the caller because it shares the session lineage with the record being stopped. Requirements: (1) confirm the matching path in the stop code; (2) stop refuses to signal its own caller or any ancestor of it, and refuses to signal a process that belongs to a different live record; (3) regression test: a fake record whose recorded session or cmdline matches the caller's own process tree; stop must not signal the caller; (4) land under the usual gate. Also decide and document whether stop of a headless record should signal anything beyond its systemd unit. (Gap 27 in a consumer team's restore plan.)
