---
title: 'agentctl-console: a console that follows an agent and accepts typed owner input'
status: closed
priority: 1
issue_type: feature
assignee: mu-rsctl
created_at: 2026-11-10T14:45:00.000000000+00:00
updated_at: 2026-11-10T14:45:00.000000000+00:00
---

# Description

The owner wants an agent console they can type to, not only watch: today `agentctl tail` is read-only output and `agentctl send` is a separate one-shot command, so following an agent and talking to it means two terminals and two tools. Build `agentctl console NAME`:

- Follow the agent's event log like `tail --follow` (the same rendering) for headless agents. For interactive (TUI) agents there is no event log; the console still accepts input for them and says so in its banner (watch the pane with `agentctl read NAME`), rather than pretending to follow.
- Read stdin lines; each line is delivered as an owner message through the normal send path (queue, receipt, the same delivery machinery as `agentctl send`), opened with the sender header `[from owner via console]`, with a visible echo of the line and the delivery outcome in the console. Owner classification must recognise the console header so these prompts get owner handling.
- A few slash commands: `/interrupt [text]` (the urgent send path; without text it delivers an urgent stop request), `/approve aN` and `/deny aN [reason]` (the headless approval answers), and `/quit`. An unknown slash command is an error shown in the console, never sent to the agent.
- Safe by construction: Ctrl-C quits the console without signalling the agent, and the console never types into the agent's pane directly (no raw pane typing); all input goes through the queued send path.

# Design

- New `console` subcommand beside `tail` in the Rust CLI, reusing `crate::headless::tail::run` for the follow rendering on a worker thread with a shared, line-locked stdout, and the same send branches `agentctl send` uses (`send_headless` / `send_identified` / `send_urgent` / `queue_identified`) stamped with sender `owner` on route `console`. Stdin is read on a reader thread into a channel so Ctrl-C (a quit flag, never a signal to the agent) and `/quit` stop both the reader loop and the tail follower cleanly.
- Extend the owner-prompt classification (`owner_prompt` in `rs/agentctl/src/agent.rs`) to recognise the console header (sender `owner`, route `console`) alongside the chat-bridge and `human@HOST` cases.
- Tests are pty-driven: a typed line is delivered (the queue record carries the console header) and echoed with its outcome, and `/interrupt` maps to the urgent send. The console is exercised against the existing fake-client fixtures, not a live harness.

# Acceptance Criteria

- `agentctl console NAME` on a headless agent renders its event log as it grows and delivers typed lines as owner messages with the `[from owner via console]` header, echoing each line and its delivery outcome.
- `/interrupt` delivers urgently; `/approve aN` and `/deny aN` answer the named headless approval; `/quit` and Ctrl-C leave the console without signalling the agent; an unknown slash command errors in the console and sends nothing.
- A pty-driven test proves typed-line delivery plus echo and the `/interrupt` urgent mapping; the focused agentctl suites pass and the slice is gated (owner priority) before landing.

# Notes

Filed 2026-11-10 on an owner request (wanted for a headless coordinator restart the same day). Related: the `tail --follow` event rendering it reuses, #671 headless approvals (the `/approve` and `/deny` answers), and #609 sender-header-mandatory (the header grammar the console header follows).

Closed 2026-11-10 as a duplicate of #670 agentctl-console (filed 2026-10-28 from the same owner request, with the governing design note at `ai_docs/transient/2026-10-28-agentctl-console/README.md`). The work continues under #670; this file's sender-framing spec is superseded by that design (console input is a plain user turn with no framing).
