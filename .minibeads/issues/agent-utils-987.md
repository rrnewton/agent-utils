---
title: 'agentctl-muse-fresh-rebind: no supported way to re-point a Muse record at a fresh session in its live pane'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-06T23:55:00.000000000+00:00
updated_at: 2026-11-06T23:55:00.000000000+00:00
---

# Description

A native Muse lane (started with `agentctl start`) got stuck: every turn its goal reminder started re-emitted the same summary and made no edits. The operator stopped it and started a fresh Muse conversation in the same pane (same argv plus `resume <new id>`). No agentctl command will then point the record at the new conversation and process:

- `agentctl anchor NAME --replace --session-id NEW` refuses: "the resumed session contradicts the recorded session; revive it instead". Without `resume` in the argv it refuses with "the foreground argv is not the recorded argv resumed".
- `agentctl bind-session NAME NEW` refuses: "custom harness \"muse\" is not the foreground process in pane ...". It checks the dead pinned PID, which only anchor would replace.
- `agentctl retire-stale-registration` refuses: "registration still matches its live pane; use normal stop for a valid registration".
- `agentctl revive --fresh` is Claude Code only, and `revive` without it resumes the stuck conversation.
- `agentctl stop` closes the tab and the fresh process.

So `agentctl send` to the lane stays pending ("custom harness \"muse\" is not the foreground process"), and the watcher reports it unreachable, while the lane's own sends work. A conversation with no turns also cannot be resumed ("has no saved log"), so even the argv-resume route needs a throwaway first turn.

# Design

- Add an explicit operator rebind: for example `agentctl anchor NAME --replace --session-id NEW --new-conversation` (or `bind-session --repin`). It checks that the pane's foreground process is the recorded harness kind, the recorded cwd and the same launch argv apart from the resume tail, and that the session's runtime file names that process. It then records the new session and PID, keeps the old session id in the record's history, and keeps the queue.
- Extend `revive --fresh` to Muse: stop the stuck process and start the recorded argv without `resume`. Record the new conversation id once Muse reports it.
- Fits beside #986 restart-when-idle, which restarts lanes in place.

# Acceptance Criteria

- A test: a Muse record whose pane now runs a fresh conversation (same argv plus a new `resume` id, or no `resume`) is re-pinned by the new rebind. A following `agentctl send` delivers.
- A test: the rebind refuses when the pane runs a different harness, cwd or argv.
- A test: `revive --fresh` on a Muse record starts a new conversation and records its id.

# Notes

Seen 2026-11-06 on one host while restarting a stuck Muse lane by hand. Interim workaround: deliver the brief by pane with an explicit sender header; the lane's replies through `agentctl send coordinator` work.
