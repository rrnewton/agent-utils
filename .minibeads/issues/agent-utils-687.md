---
title: 'tmux-wrapper-identity: tmux launches fail when the harness command is a wrapper script that execs another binary'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-29T13:46:45.815446892+00:00
updated_at: 2026-10-29T13:46:45.815446892+00:00
---

# Description

[opus 5.5] On the tmux terminal backend, `agentctl start` cannot start a harness whose command is a
wrapper script that execs a different binary. agentctl pins the resolved harness program
(its device and inode, or the shebang interpreter running the script) and waits for that program
to be the pane's foreground process. Once the wrapper execs the real binary, the foreground
process is neither the pinned image nor the interpreter running the script. So the launch fails
after the startup timeout with "<harness> was not the foreground process in pane %N after 30s",
although the harness is running and ready, and the record is left `launch_failed`. Reproduced on
2026-10-29 with Claude Code, where the installed `claude` command is such a wrapper, with and
without `launch_env` (found while testing #685 agent-env-path). The Herdr backend is not affected
for `herdr agent start` kinds, because Herdr detects the agent itself.

Possible directions: accept a foreground process that descends from the pinned wrapper's pid with
the same session, or resolve the wrapper's final exec target (configurable per harness) before
pinning. The process identity must stay strong enough that an unrelated program in the pane
cannot be adopted.
