---
title: 'agentctl-start-pane-busy: start must survive agent_pane_busy on the pane it just created'
status: closed
priority: 0
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-02T17:01:15.479002890+00:00
updated_at: 2026-11-02T17:01:15.479002890+00:00
closed_at: 2026-11-02T17:01:15.479002890+00:00
---

# Description

[opus 5.5] On one host every `agentctl start` since 2026-11-02 15:40Z failed with `agent_pane_busy: agent target pane P is not an available shell`, naming the brand-new tab's own pane. The terminal server's log shows the shape each time: `tab create`, then a single `agent start` about 0.36 s later, refused within 5 ms, and no retry. Starts on the same server had succeeded through 2026-11-01, when panes spawned in about 10 ms; now a pane takes 0.3-0.4 s to spawn.

Root cause in agentctl: `start` handed the created pane straight to `herdr agent start` and treated one `agent_pane_busy` as fatal. Herdr refuses a pane whose shell is not alone in its foreground process group, or that it believes holds an agent, and decides that before typing anything. Its client-side retry covers only 2 s of an initializing shell, and on that host it did not engage. A few seconds later the same panes were ordinary shells at their prompts.

Ruled out: the login shell's startup stderr (`bash: ~/.vercel.env: No such file or directory`) and the commands its startup files run. Bash takes the terminal for itself 3 ms after exec, and nothing in startup takes it back (strace on both hosts). Also ruled out: the Herdr version (0.9.3 and 0.9.5 share this code), and picking a pane by label. `start` uses only the root pane of its own `tab create`. In one capture someone typed into the new tab after the failure.

Fix: when Herdr answers `agent_pane_busy`, `start` checks the same pane again. That means the same pane id and the same terminal, and an end to the wait if the pane closes or its terminal is replaced. It waits until no agent is reported there and the pane's shell is alone in its foreground group on two looks in a row, then offers the launch again, all within `--startup-timeout`. If the pane never frees, the error names what held it (foreground group and processes, or the agent Herdr reports). Regression tests: a noisy login shell (startup children, then a foreground job), a slow and initially failing process report, a pane that never frees, a replaced terminal, and a busy pane in another tab with the same label. `start` only ever launches into the pane its `tab create` returned.

Verified live with Herdr 0.9.5: a startup file that holds a foreground job for 6 s reproduces the exact refusal with the deployed agentctl. The fixed build is refused once and then starts Claude in the same pane 6.5 s later.

Unverified: which transient Herdr state produced the refusals on the affected host was not identified (only read-only access there). The fix covers any refusal that clears within the startup timeout. If one does not, the new error says what the pane held. The Python edition is unchanged: it is not on the production start path, and no cross-differential case exercises this refusal.
