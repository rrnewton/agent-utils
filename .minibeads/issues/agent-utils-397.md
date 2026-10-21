---
title: 'watch-scrolled-pane: watch-agents reports a Claude Code pane scrolled up as stalled'
status: closed
priority: 2
issue_type: bug
created_at: 2026-10-21T00:12:16.586166250+00:00
updated_at: 2026-10-21T00:12:49.592477578+00:00
closed_at: 2026-10-21T00:12:49.592477447+00:00
---

# Description

[opus 5.5] On 2026-10-20 `agentctl watch-agents` (#381 agent-watch, with #393 watch-agents-background-idle's running-work rule) reported a Claude Code agent as "working, or idle while its screen shows work running, with no new output for over 30 minutes (... screen unchanged)" while it was actively working. Its view was scrolled up: Claude Code freezes the transcript where the reader left it, draws a "─── N new messages ───" divider, and ends the last row above the input box with "N new messages (ctrl+End) ↓" while new output arrives below the view. A background validation run was going (footer "1 shell").

The watcher judges change from the text it reads, and a Claude Code pane keeps no terminal scrollback (Herdr reports max_offset_from_bottom 0), so nothing below the view can be read: not the running turn's timer, which ticks every second at the bottom, not the newest output, and not a closing DONE:/BLOCKED: verdict. A growing count already changes the hashed screen, but a count that holds still proves nothing: one long tool call or sub-agent run is a single message. The same scroll also hides the verdict, so a finished agent left scrolled up would be reported idle-no-verdict.

Fix: read the pill from the end of the last row above the input box; while it shows, judge neither stalled nor idle-no-verdict (an episode not yet reported is dropped, one already reported is kept so it is not sent twice), list the agent under "scrolled" in the pass's JSON, and start the clocks again when the view returns to the bottom. Approval prompts are still reported. Fixture tests come from live pane captures.
