---
title: 'herdr-blank-grid-claude: long-running Claude panes show an empty grid while the agent works'
status: open
priority: 1
issue_type: bug
assignee: cl-mq@devbig030
labels:
- agentctl
- herdr
created_at: 2026-11-03T23:02:25.765784142+00:00
updated_at: 2026-11-03T23:02:25.765784142+00:00
---

# Description

[opus 5.5] Several long-running native Claude Code panes on one host show an EMPTY Herdr grid while the agent is alive and working: agentctl's drain and send refuse ("composer_unreadable: ... does not show a recognisable claude composer ... ansi_blank=true ... working=true"), yet `herdr pane send-text` plus Enter still reaches them. Seen 2026-11-03 on a coordinator pane (Claude process running ~30 h, its binary replaced on disk since, a 272 MB transcript, ~88k small writes/s; the title spinner keeps updating; one Ctrl+L redraw did not restore the grid) and on two lanes of another team on the same host. Every message to the affected agents parks; #834 only escalates the refusals, it does not explain or cure the blank grid.

# Design

Read-only first, then fix:
- Census: which panes on both hosts are blank right now (grid non-blank character count, scrollback, agent kind, process age, binary path and whether the on-disk binary changed since start).
- Hypotheses to test: the Claude Code binary or install replaced under long-running processes (version, mtime of the install vs process start); a Herdr grid-reset bug (alternate screen, synchronized output, scroll region, a missed full repaint); a resize (SIGWINCH via `herdr pane resize`) restoring the grid; what `herdr pane read` and the raw grid show.
- A safe recovery that needs no restart if possible (e.g. a resize or a Herdr re-attach), documented; otherwise the safe restart order (TERM the native child first, wait for the wrapper, confirm the shell is the foreground group, relaunch with --resume, then `agentctl anchor --replace`).
- The agentctl side: detect the blank-grid state (alive harness, empty grid) explicitly, try the safe recovery once, and report it as its own finding.

# Acceptance Criteria

- The census and the root cause are written in this issue.
- The recovery is shown on a real blank pane (or the reason it cannot be, with the restart procedure).
- agentctl reports the blank-grid state by name, with a test from a captured state.
