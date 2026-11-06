---
title: 'chat-bang-commands: owner-only ! commands in the chat bridge to list and talk to sub-agents'
status: open
priority: 1
issue_type: feature
assignee: '014: mu-bang'
created_at: 2026-11-06T01:32:43.596497697+00:00
updated_at: 2026-11-06T01:32:43.596497697+00:00
---

# Description

The owner wants to talk to sub-agents directly from the chat channel, through owner-only, deterministic "!" commands that the bridge handles itself and never delivers to an agent (owner request 169).

# Design

- `!help`: lists the commands.
- `!who`: the managed-by tree, followed transitively, split into Pinned and Ephemeral agents, showing each one's name, harness and model, state and host. `!who NAME` shows one agent's detail.
- `!@NAME <msg>`: opens a new Slack thread bound to that agent's pane. The message goes through the normal admission, receipt and reply protocol for that agent's harness, and replies are posted back into the thread.
- `!end`: unbinds the thread. A thread also unbinds automatically when its agent stops.
- ONE notice to the parent coordinator per bound thread, not one per message.
- Only the owner can run these commands; anyone else gets a refusal, and nothing is delivered.
- v1 uses the local registry. Cross-host lieutenants via agent-teams come in v2.

# Acceptance Criteria

- Tests for each command, including a non-owner refusal, and that a "!" line is never delivered to the coordinator as a prompt.
- A bound thread: an owner message there reaches the named agent with a receipt only on evidence (#896 rules), and its reply posts into the thread.
- Auto-unbind on agent stop, and exactly one parent notice per bound thread.
- Help and the userguide document the commands.

# Notes

Owner request 169, relayed by the lead on 2026-11-05 at 9:04 PM ET (P1). A lane on the lead host (Muse ultra) is designing it and implements it there once that host can create slots again. It touches chat_runtime.rs, the same area as #852 D and #900 (cl-start), so coordinate.
