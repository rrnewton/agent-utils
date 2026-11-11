---
title: 'chat-thread-bind: bind an existing chat thread to a sub-agent'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-11T17:55:23+00:00
updated_at: 2026-11-11T17:57:31+00:00
---

# Description

[watermelon-full-preview] The owner wants to talk directly to a coordinator's sub-agent, especially a lieutenant, without creating a new channel (owner rush replies 392-395). #917 chat-bang-commands already opens a NEW bound thread with `!@NAME`; reply 393 narrowed this issue to that fresh-thread model. What is missing is a coordinator-side way to open such a thread on a plain-words ask, when the owner forgets the special command.

# Design

- Extend #917, do not fork: bindings stay in `thread-bindings.json`, requests keep `target_agent`, replies keep the existing chat reply flow into the bound thread, and the parent notice stays exactly once per bound thread.
- `agentctl chat open-thread --agent NAME --text "..."` is the coordinator-side equivalent of `!@NAME <text>`: it posts the root message as the bridge, binds that new thread to NAME exactly as `!@NAME` does, admits the owner's text as the bound agent's first request, and prints the thread id.
- On a plain-words ask ("let me talk to liteinst", "bind liteinst here", "open a thread with museli-lt") the coordinator resolves the name through its managed-by tree (registry lineage plus its own notes; never a guess, and it asks when ambiguous), runs open-thread with the owner's own words as the first message, and otherwise replies with the exact `!@NAME <text>` line for the owner to paste. This goes in both the agentctl-chat skill and the coordinator skill.
- The bang `!help` text (HELP_TEXT) says the owner can also just ask their coordinator in plain words (e.g. "open a thread with liteinst").
- Shelved unless the owner asks again: binding an EXISTING thread (`!bind`), rebind-moves, and one-thread-per-agent enforcement.

# Acceptance Criteria

- `chat open-thread` posts the root, creates the #917 binding, delivers the owner's text only to the bound agent, the agent's reply lands in the new thread, and the parent notice is sent once. No wall-clock budgets (owner rule 346).
- A scripted walk of the plain-words path (fake bridge plus registry: ask -> name resolution -> open-thread called with the owner's text; plus the `!@NAME <text>` fallback line) passes.
- A deployable prototype lands on main today for testing against another team's bridge on another host; this build never touches that bridge, its units, or its registry.

# Notes

Owner rush replies 392-395, relayed by coordb. Prior art: #917 chat-bang-commands (b6fb7609, 9e9b1bdb, 53bd4459) and ai_docs/transient/2026-11-06-chat-bang-commands-design.md. Cross-host bindings remain v2.
