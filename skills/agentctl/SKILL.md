---
name: agentctl
description: Start, inspect, message, and retire persistent named coding-agent sessions, including owner-configured local launch profiles and agentcloud workers on their own nodes. Use when asked to create a managed Codex, Claude, Muse, agentcloud, or other configured subagent.
---

# agentctl

Use the installed command as the authority:

- `agentctl quickstart`
- `agentctl start --help`
- `agentctl profiles --cwd WORKDIR`
- `agentctl status NAME`
- `agentctl send NAME --file PROMPT`
- `agentctl read NAME --lines 500`
- `agentctl move NAME`
- `agentctl userguide`

**Every agent launch and every agent inspection goes through agentctl.** Do not
start, split, move, message or read agent panes with raw `herdr agent start`,
`herdr pane split` or `herdr pane move`. Raw `herdr agent start` without `--tab`
SPLITS the caller's current tab. It also skips the launch profile, so the new
agent comes up without its owner-configured flags. agentctl creates a labelled
tab in the configured workspace, and it moves a split agent into its own tab.
If agentctl itself fails or lacks a capability, REPAIR agentctl in agent-utils
and land the fix. Do not route around it with raw herdr calls. Exception: the
owner explicitly asks for a raw operation.

To start a named local profile, first list the profiles in the intended working
directory, then select one exactly:

```sh
agentctl profiles --cwd /work/project
agentctl start reviewer --cwd /work/project --profile muse-watermelon
```

Profiles are owner-controlled launch policy. Never invent, add, remove, or
rewrite harness arguments, environment entries, trust flags, permission-bypass
flags, models, or reasoning effort. If the requested profile is absent or
refused, report that failure instead of approximating it with ad hoc flags.

A project may pin interactive agents to a Herdr workspace label with the
top-level `workspace` key in `.agentctl/profiles.json`. Configured policy wins
over an inherited `HERDR_WORKSPACE_ID`; do not add `--workspace-id` unless the
owner requested an exact destination that resolves to the configured label.
Automated input and terminal reads refuse a misplaced pane. Observational
`status`, `attach`, `goal` query, and `wait` remain available, as does `stop`
when no move is pending.

Use `agentctl move NAME` to move an existing running, agentctl-owned native
Herdr agent into the configured workspace without restarting its process or
conversation. The destination must already exist. Adopted and custom-pane
sessions cannot be moved. If status reports an incomplete move, rerun the same
move command; stop refuses until recovery commits the new pane identity. A
pending move remains recoverable even if the project `workspace` key was
removed: the rerun uses the exact destination workspace ID stored in the
durable intent and does not choose a new destination.

When a task needs its own machine, such as a checkout that must not be created
locally, and `agentctl capabilities` lists `agentcloud`, start an agentcloud
worker: an owner profile with harness `agentcloud`, or
`--harness agentcloud --cloud-harness claude-code --provision --envspec NAME`.
`agentctl` creates the session, records its ID, and opens `agentterm` in the
worker's tab. Read its reply with `agentctl read NAME --output last`; `stop`
halts and archives the session but does not release the node reservation.

Use a fresh name for a fresh task. A successful `start` returns durable session
identity; it does not prove the initial prompt completed. Use `wait`, `read`, and
`status` to inspect progress. Pause automation before a human types in the same
terminal.

Prompt delivery is conservative. Exit 75 means the message is safely pending
and may be retried with `agentctl drain NAME`. Exit 76 means delivery may have
crossed the submission boundary; inspect the pane and failed artifact before
doing anything. Never resubmit a possibly-submitted prompt automatically.

`agentctl stop NAME` retires only the exact owned runtime after identity checks.
Do not remove registry files or panes by hand to work around a refusal.
