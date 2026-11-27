---
title: 'agentctl-coordinator-lean-headless-profile: a coordinator-lean headless launch allows only short read-only commands and caps every Bash call at about 30 s, with a helper lane as its hands'
status: open
priority: 1
issue_type: feature
assignee: cl-leanprofile
labels:
- agentctl
- headless
- coordinator
created_at: 2026-11-26T19:20:00-05:00
updated_at: 2026-11-26T19:20:00-05:00
---

# Description

A headless Claude coordinator ran Bash calls of more than 400 s while the owner's messages waited behind them at queue position 10 and beyond. A coordinator's job is to answer the owner and steer lanes; long work (builds, test runs, wide searches, scripts) belongs to a helper lane. The owner (request 389): a coordinator must never make big Bash calls; "It should use a helper agent as its hands." The owner also could not tell what an interrupt does to the queued messages.

Nothing in agentctl today stops a headless coordinator from running a long command: a launch profile passes whatever harness arguments and settings it carries, and Claude Code's own Bash limit defaults to minutes, not seconds.

# Design

A `coordinator-lean` option for headless launches (a launch-profile field, also a `run-headless` flag), which agentctl turns into harness settings it owns:

- A permission set that allows only short read-only commands (file reads and searches, `git log/show/status/diff`, `agentctl status/read/tail/list`) plus `agentctl send` to its own lanes and the agent-teams send/reply verbs, and denies writes, installers, network tools and privilege escalation. Anything else is refused at once (no permission prompt waits for a human).
- An enforced per-Bash-call limit of about 30 s: the harness's own Bash timeout ceiling set through the launch environment, background Bash disabled, and a pre-tool hook that refuses a call asking for a longer timeout or for background execution, so a 400 s call cannot happen.
- Owner messages keep their immediate (deliver-now) delivery.
- A documented "hands" pattern: the coordinator starts or keeps a helper lane, sends it the long command with a reply-to, and gets a short result back as an ordinary queued message.

The design note (in the harness repository's plans) names every file, the exact settings and how they are tested.

# Acceptance Criteria

- `run-headless` with the option (or a profile that sets it) launches Claude with the lean permission set and the Bash limit; `agentctl status` shows that the agent is lean.
- On a scratch headless agent in a scratch registry: a command that needs 31 s is refused or cut at the limit, a long-timeout or background request is refused, and `agentctl send`, `read` and `status` still work from inside the agent.
- Unit tests for the generated settings and hook; the option is refused for a harness that cannot enforce it.
- A ready-to-apply profile and restart steps for an existing headless coordinator.
- The agentctl docs gain a short section on turn-boundary versus immediate delivery, what an interrupt does to the running tool call and to queued messages, and each "cancelled" state shown by `agentctl tail`.

# Notes

Filed for owner request 389 (2026-11-26). Code waits for the coordinator's GO on the design note.
