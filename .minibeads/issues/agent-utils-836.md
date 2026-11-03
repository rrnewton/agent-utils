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

# Notes

[opus 5.5] 2026-11-03 findings (cl-mq). Times are ET; the host logs PST.

## Census

devbig030, 31 agent panes. All 6 blank panes are Claude Code panes started 11-02 11:00-20:00 ET, each with its binary replaced on disk since it started: wJ:pB8, wJ:pNP, wJ:pNR, wJ:pPG, wJ:pQ4, w16:pD. In each, grid non-blank = 0 and scrollback = 0.

Healthy panes:
- three Claude panes started today after the 16:48 ET upgrade;
- all 22 codex/muse panes, although their binaries were also replaced.

The devbig014 census, taken by its coordinator, shows a replaced binary is necessary but not sufficient:
- 11 of its 12 pane-bound Claude processes run a deleted executable, yet only the 4 oldest are blank (reviewer panes started 10-25/10-26);
- processes started from 10-28 on survived up to 14 upgrades.

## Where the fault is

It is in Claude Code, not Herdr:
- Herdr drains every pty: the output queue (TIOCOUTQ) was 0 for all panes.
- strace of the blank coordinator's native process (w16:pD) while it worked showed only OSC 0 title writes (the spinner); no frame was written.
- A healthy Claude writes synchronized-output frames (`ESC[?2026h` ... `ESC[?2026l`) many times a second.
- On a real resize (pty rows 50 → 49 → 50), the blank process received SIGWINCH, read the new size (TIOCGWINSZ) and wrote nothing.
- So its renderer is dead while the agent loop and the title timer still run.

## Trigger window

On devbig030 the other team's blank panes went blank together. The last confirmed agentctl deliveries to two of them were at 16:21 ET, and the first composer refusal (a third) came at 16:39 ET.

Inside that window, at 16:26:55 ET, dotsync brought in updates to two peer-session records in `~/.claude/sessions/` (pids 2812651 and 2885529). They come from another host and boot (version 2.1.250, started 2026-09-03). `~/.claude/sessions` is synced across hosts by dotsync, and Claude instances watch that directory for peers.

Leading hypothesis, unproven: an incoming peer record kills the renderer of long-running instances. The fb-claude-code upgrade came later, at 16:48 ET, so it is not the trigger.

## Not reproduced

A Claude copy whose binary was replaced the way rpm does it kept repainting on resize and on /help.

## Recovery

- No restart-free recovery was found. `herdr pane resize` is a no-op on a single pane; a same-size SIGWINCH and a real pty resize both had no effect.
- The cure is a child-first restart with `--resume`:
  1. TERM the native child first.
  2. Wait for the wrapper to exit.
  3. Confirm the shell is the foreground process group.
  4. Relaunch with `--resume`.
  5. Run `agentctl anchor --replace`.

## agentctl

- Every `composer_unreadable` refusal now names the state `blank_grid` (`submission::BLANK_GRID_STATE`, `is_blank_grid`).
- The refusal carries `harness_binary_replaced` and the cure.
- An idle blank harness whose binary was replaced is now classified too; before, only `working=true` was.
- The `blank-working` watch alert names the state and the cure.
- agentctl does not try a resize first, because none restored the grid.
- The test fixture is `testdata/blank-grid/claude-renderer-silent.txt`, captured from w16:pD.

Left open for the trigger's proof (and an upstream report) once a blank episode can be caught as it starts.
