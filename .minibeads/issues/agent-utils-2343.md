---
title: 'agent-follows-reattach: an agent record follows its pane across a viewer re-attach, with no manual anchor --replace'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-11-26T21:25:00-05:00
updated_at: 2026-11-26T21:25:00-05:00
---

# Description

An agent adopted with `--harness unknown` (adapter herdr-foreign) whose Herdr pane shows a foreign TUI through a tmux viewer (the pane runs `tmux attach -t VIEWER`, and the TUI itself runs in another tmux server) is pinned to the pane's foreground process. That process is the `tmux attach` client, not the agent. When the TUI restarts, the old viewer session goes away with it, so its attach client exits. A new viewer is then created and the same Herdr pane re-attached. From that point every input to the record is refused and held:

```
outcome: pending
error: message ... remains pending without consuming a retry attempt: refusing input to agent 'AGENT':
the anchored unknown process (pid N) has exited, and pane PANE now runs another unknown ...
```

The chat bridge targets the same record, so owner chat messages are held the same way until someone runs `agentctl anchor AGENT --replace` and then `agentctl drain AGENT`. Nothing is lost, but delivery stops with no notice. In the #2108 trial, after a planned rename with a stop and resume of the orchestrator, input was held for about 35 s until a manual re-anchor; any restart while nobody is watching would hold the owner's messages indefinitely.

Repro:
1. Run a foreign TUI in its own tmux server. Create a viewer session holding a linked copy of its window: `tmux new-session -d -s VIEWER -n placeholder 'sleep infinity'; tmux link-window -s TUI:WINDOW -t VIEWER:9; tmux kill-window -t VIEWER:placeholder`.
2. In a Herdr pane run `tmux attach -t VIEWER`, then `agentctl adopt AGENT --harness unknown --pane PANE --workspace WS --cwd DIR`. `agentctl send AGENT --file F` delivers.
3. Stop and resume the TUI. Its window, and so VIEWER, disappears and the attach client in PANE exits. Recreate VIEWER as in step 1 and run `herdr pane run PANE "tmux attach -t VIEWER"`.
4. `agentctl send AGENT --file F` now returns `outcome: pending` with the error above.
5. `agentctl anchor AGENT --replace` re-pins `harness_pid` to the new attach client, and `agentctl drain AGENT` delivers the held message.

# Design

- When the anchored process has exited and the pane's new foreground process has the same executable and the same argv as the recorded one (here `tmux attach -t VIEWER`) in the same pane and terminal, re-anchor automatically, record that in the agent's event log, and deliver. Refuse as today when the executable, argv or terminal differ.
- Optionally, let an adopted unknown-harness record declare its pane process a re-attachable viewer (an adopt flag stored in the record), so a different viewer name is also followed while the pane and terminal stay the same.
- While input is held for an anchor mismatch, `chat health` and `agentctl status` say so (held since, how many messages, the one command that fixes it) instead of reporting OK.

# Acceptance Criteria

- Following the repro, step 4 delivers without a manual anchor, and the record's `harness_pid` names the new attach client.
- A pane whose new foreground process is a different program (a shell, another harness) is still refused, with the current error.
- A test of a held message for an anchor mismatch shows it in `chat health` and `status`.
- The harness support matrix notes the behaviour for unknown-harness viewers.

# Notes

Found in the #2108 debugging pass on 2026-11-26 (owner request 392), during a planned stop and resume of the trial's orchestrator session. Workaround, now in that team's restart recipe: after recreating the viewer and re-attaching the pane, back up the agent record and run `agentctl anchor AGENT --replace`, then `agentctl drain AGENT`. Not tested: whether `chat health` already flags the held state (it ran only after the re-anchor).
