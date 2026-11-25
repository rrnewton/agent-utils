---
title: 'agentctl-headless-wrapper-keeps-harness-stderr: the wrapper records the last lines of the harness''s stderr on every exit'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- headless
assignee: cl-museserve
created_at: 2026-11-25T12:55:00-05:00
updated_at: 2026-11-25T12:55:00-05:00
---

# Description

A headless Muse on the serve transport exited 2 on every restart until the wrapper stopped with `restart-limit` (#2247). Its event log held only the lifecycle `harness_exited {code: 2}` events, with nothing of the harness's stderr. The cause, a usage error Muse printed on stderr, could only be found by running the harness by hand.

The wrapper starts the harness with stderr inherited (`rs/agentctl/src/headless/wrapper.rs`, `spawn_harness`). Whatever the harness says when it fails goes to the wrapper's own stderr, at best a journal, and never reaches the event log, `tail` or `status`.

# Design

- Pipe the harness's stderr through a reader thread. The thread copies every byte on to the wrapper's own stderr, so the journal keeps everything as before, and keeps a bounded tail of lines: at most 20 lines, each cut to 512 bytes, at most 4096 bytes in all. The work per line is constant and no buffer grows without bound.
- On every harness exit, after a short wait for the reader to reach end of output, the `lifecycle.harness_exited` event gains an optional `stderr_tail` field: `{"lines": [...], "omitted_lines": N}`. The lines are oldest first and masked with the existing output redaction (`mask_output_line`). `omitted_lines` counts the earlier lines that fell out of the bound. The field is absent when the harness wrote nothing to stderr, so readers that do not know it ignore it.
- `agentctl status` for a headless agent shows the most recent harness exit in its recent event window as `last_harness_exit` (sequence, code, next step and stderr tail). `tail` prints the tail lines under the exit line.

# Acceptance Criteria

- A fake harness that writes to stderr and exits 2 shows those lines in the `harness_exited` event and in `agentctl status`.
- The tail stays within its line and byte bounds when the harness writes far more than that.
- Credential-shaped text in the harness's stderr is masked in the event.
- The wrapper's own stderr still carries all of the harness's stderr.

# Notes

The new event field is a persisted change, so its exact shape went to the lead coordinator before landing: an agent can be read by an older agentctl, which must ignore the field. Related: #2247 agentctl-headless-serve-muse-argv.
