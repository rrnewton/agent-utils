---
title: 'orc-restart-resume-caveats: an Orc session restart is queued, background work re-runs, and an interrupted turn is redone'
status: open
priority: 2
issue_type: task
labels:
- agentctl
- chat
- restore
created_at: 2026-11-26T19:35:00-05:00
updated_at: 2026-11-26T19:35:00-05:00
---

# Description

A scratch Orc session (current release, its own tmux socket, nothing shared with a live team) was tested on 2026-11-26 while it streamed a long reply, with two inputs queued (one typed into its TUI the way the chat bridge types, one posted to `/api/message` with an idempotency key):

- `orc session restart` returned ok at once, but the restart was itself queued as a `/restart` input behind the two pending inputs. Both were answered once, and the restart ran about 30 s later when the session went idle. A caller that trusts the exit status (turnover, restore, a coordinator) reports a restart that has not happened yet, and a session that never goes idle never restarts.
- A background workflow (a 100 s script) was re-attached after the restart and ran again from the start: background work that is not idempotent runs twice.
- A hard `orc stop` mid-turn followed by `orc session resume` kept the partial reply and both queued inputs, which were each answered once, but the model redid the interrupted turn from the beginning. In a chat-bridged coordinator, a reply already posted in that turn can be posted again: the bridge accepts a second reply to the same request as ordinal 2.

Queued input survived in both cases. The other three behaviours matter to agent-utils wherever it drives an Orc-run team.

# Design

- Turnover and restore of an Orc target wait for the restart to happen (a new engine process or boot id, read from `orc session info` or the session socket) with a bounded wait, and report "queued, not yet restarted" instead of success while it is pending.
- The Orc team's chat-reply wrapper refuses a second reply to a request that already has one unless asked to add one, and the bridge flags a reply whose text repeats an already-sent reply to the same request.
- The harness support matrix (unknown TUI row) and the Orc team notes record the three behaviours, with the advice to make background scripts idempotent.

# Acceptance Criteria

- A turnover or restore test against a fake Orc whose restart is deferred reports pending, then success only after the process changes.
- A test of the reply guard: a second reply to an answered request is refused by default and flagged when forced.
- The matrix and the team notes describe the restart, background re-run and redone-turn behaviours.

# Notes

Found in the #2108 debugging pass (owner request 392, 2026-11-26); scratch session stopped and its files removed afterwards. Related: #2325.
