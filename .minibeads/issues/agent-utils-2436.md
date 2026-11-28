---
title: 'agentctl-peer-detector-harness-gate: the watcher reports a non-Muse pane quoting the Muse peer dialog as waiting at a peer prompt'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-27T23:08:07-05:00
updated_at: 2026-11-27T23:08:07-05:00
---

# Description

[opus 5.5] Found while landing #2421 agentctl-asks-never-block
(2026-11-27, about 9 PM ET): a running watcher reported a working
Claude Code lane as waiting at a Muse peer-approval prompt. The lane
had printed, in an ordinary tool output, the doc comment of the
detector itself, which names a `Message from <peer>` heading and all
four choices. `peer_approval_prompt_line` (#974
muse-peer-approval-prompt) reads the bottom rows of any screen for
that heading and two of the choices, whatever the harness and whether
or not an editor is drawn, so the quote was enough. The coordinator
got a false blocked notice. No key was sent: the detector only
reports, and since #2421 the deny sweep never refuses the peer dialog
and picks its recognisers by the agent's harness.

# Design

- Gate the detector on the screen's harness: only a Muse pane (or an
  unknown harness, which has no recogniser of its own) can be at the
  peer dialog. The callers that know the harness pass it; the quick
  check reads it from the record as the editor sweep does.
- For a Muse pane, require the dialog to stand where the editor would
  be: no Muse editor rows drawn, and below the choices only Muse's
  hints, its closing rule and its status line, as the #2421 command
  form recogniser already requires.
- Keep the #974 detection of the real dialog, focused or not.

# Acceptance Criteria

- The recorded fixture `claude-quoting-muse-peer.txt` (added by
  #2421) is not reported as a peer prompt for a Claude pane, on the
  full pass or the quick check.
- A Muse pane quoting the dialog above its idle editor is not
  reported either.
- The recorded peer dialogs (focused and unfocused) are still
  reported for a Muse pane.
- Focused Rust suites pass on the gated tree.

# Notes

Reserved by the coordinator during #2421; reporting only, no key
path is involved.
