---
name: agent-harness-environment
description: What an agent's tool commands inherit from their harness (Claude Code, Codex, Muse) and terminal host (Herdr, tmux), and tested recipes to put a team's own tool directory first on PATH at launch or for agents that are already running. Use when deciding which agentctl, wrkslots or dagrun an agent will run, when a PATH or environment change does not reach an agent's tool calls, or when planning how to launch agents with a team's environment.
---

# agent-harness-environment

The reference is [`common/docs/agent-harness-environment.md`](../../common/docs/agent-harness-environment.md).
Read it before you change how agents are launched or what their `PATH` holds. The short version:

- An `export` inside one tool call never reaches the next one: every harness here starts a fresh
  shell per call.
- A `PATH` exported before launch survives in Claude Code and Muse. Codex re-sorts it through a
  login shell unless you pass `-c allow_login_shell=false`.
- Herdr hands a pane's `PATH` to an interactive shell, whose `~/.bashrc` may put other directories
  first. tmux replaces a `-e PATH=...` with its server's `PATH`. Put the prefix on the harness
  command line (`env PATH=... harness`), not in a profile's `env`.
- `BASH_ENV=<team env file>` at launch is re-read before every tool command in every harness
  tested, so one edit reaches every agent launched that way.
- A running Claude Code agent can be moved with a `CwdChanged` hook in its project
  `.claude/settings.local.json` that writes `. <team env file>` into `$CLAUDE_ENV_FILE`, plus one
  `cd` by the agent.
  A running Muse agent cannot be changed; relaunch it with resume.
- For agents started by `agentctl`, set a top-level `launch_env` (`env_file`, `path_prepend`) in
  the registry's `.agentctl/profiles.json`; `agentctl` types the environment line into each new
  pane after the shell's startup files, and adds `-c allow_login_shell=false` for Codex.

To check which binary a name resolves to, run `command -v <tool>`. The tracked launchers in
`common/bin/` also print a provenance line on stderr. For a harness not covered by the reference,
run its "Verify any harness" recipe with a throwaway agent. Never experiment on running agents
that belong to someone else.
