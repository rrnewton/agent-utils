---
name: agentctl
description: Start, inspect, message, and retire persistent named coding-agent sessions, including owner-configured local launch profiles. Use when asked to create a managed Codex, Claude, Muse, or other configured subagent.
---

# agentctl

Use the installed command as the authority:

- `agentctl quickstart`
- `agentctl start --help`
- `agentctl profiles --cwd WORKDIR`
- `agentctl status NAME`
- `agentctl send NAME --file PROMPT`
- `agentctl read NAME --lines 500`
- `agentctl userguide`

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
