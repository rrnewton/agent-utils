---
title: 'agentctl-coordinator-lean-headless-profile: a coordinator-lean headless launch allows only short read-only commands and caps every Bash call at about 30 s, with a helper lane as its hands'
status: closed
priority: 1
issue_type: feature
assignee: cl-leanprofile
labels:
- agentctl
- headless
- coordinator
created_at: 2026-11-26T19:20:00-05:00
updated_at: 2026-11-26T20:10:00-05:00
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

## Resolution

Delivered in the Rust edition:
- **Profile and rendering.** The `coordinator_lean` profile key (`true` or `{bash_timeout_seconds, extra_commands, lanes}`; Claude only; implies `no_mcp_servers`). `rs/agentctl/src/lean.rs` renders `REGISTRY/lean-settings/NAME.json` at `start`, at every headless wrapper launch, and at every interactive relaunch (`finish_launch_plan`). The file holds the Bash ceiling (`BASH_MAX_TIMEOUT_MS` / `BASH_DEFAULT_TIMEOUT_MS` with `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`), the tool and Bash deny rules, and the PreToolUse hook `agentctl lean-hook ... || exit 2`, merged with the profile's own `--settings`.
- **The hook.** It allows short read-only commands, the read-only git subcommands, `agentctl status/list/read/tail` and `chat sent/thread/help`, `send --queue-only` and `interrupt` to the agent's own lanes (managed_by chain or `lanes`), and agent-teams send/reply. It refuses everything else with a reason naming the hands lane. It takes about 3 ms per call.
- **Status and the Python edition.** `status` shows `coordinator_lean`. The Python edition refuses to launch a lean profile.
- **Docs.** The user guide's headless section and the harness support matrix ("Coordinator-lean launch", and "Headless delivery, interrupt and cancelled inputs").
- **Live proof.** A scratch headless lead on Claude Code 2.1.319 (haiku), on this build:
  - `wc -c /dev/zero` was killed at 30 s (exit 143).
  - `sleep 31`, `timeout: 120000`, `touch`, `curl` and a send to a non-lane were refused by the hook; `run_in_background` was not offered; the Write tool was absent.
  - `git log`, `rg`, `agentctl status`, `agentctl tail` and `agentctl send --queue-only` to the hands lane worked.
  - The hands lane's `send --to-manager` reply came back as the lead's next turn.

Not yet verified: the trial on a real coordinator (a consumer team's less critical coordinator first), which that team runs from the profile and restart steps handed to them.
