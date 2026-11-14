---
title: 'agentctl-unsubmitted-paste-backstop: pasted prompts left unsubmitted; watcher backstop blind in busy panes'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-14T18:31:00.000000000+00:00
updated_at: 2026-11-14T21:20:00.000000000+00:00
---

# Description

Owner 069 (2026-11-14): owner and lane/coordinator prompts sat pasted in
TUI input boxes, unsubmitted, for up to ~1.5 h; the owner pressed Enter by
hand. Owner requests reached the liaison 1 h–1 h 38 min late; the
coordinator's agentctl queue held up to 58 pending prompts; a Muse lane
had a pasted message left in its editor.

Cause (reported to the coordinator before fixing, from live read-only
evidence): the watcher's idle draft sweep (`sweep_editor_drafts`,
#731) runs every pass but its first gate,
`composer_view(harness, screen) else continue`, silently skips any pane
whose composer it cannot parse. For Muse that is every pane, always:
the Muse adapter never overrides `composer_view`, so the default returns
`None`. For Claude it is any busy screen the framed view cannot split
(only a `❯` row between two visible plain rules counts), which is the
state of the liaison and coordinator panes when their queues jammed:
watch.log repeatedly records "does not show a recognisable claude
composer after 15s; nothing was typed". The sweep therefore never
classified, flushed, or reported a single draft (`draft_sweeps: []` all
day). A second hole: the sweep attributes with `Vouching::Recent` while
the flush re-checks Claude/Codex drafts with `Vouching::Unproven`, so a
draft the sweep calls ours can fail the flush's stricter match and loop
as flush-failed instead of being submitted.

# Design

- The sweep reads a Muse editor draft with the Muse editor reader
  (`muse_verified_process_editor_draft`) instead of the generic
  `composer_view`, so Muse drafts are visible, attributed, and flushed
  like other harnesses'.
- Claude's framed view gains an unframed fallback: a `❯` row above the
  composer's bottom rule with no rule between counts as the composer
  (a tall draft pushes the top rule off screen). Both editions.
- The sweep attributes each draft with the vouching its flush will use
  (Muse `Recent`; Claude/Codex `Unproven`), so "ours" means the same
  at both steps.
- A pane the sweep cannot parse while its queue holds an untyped or
  stranded prompt is a `draft: unreadable` finding, reported blocked
  once after the sweep age — never a silent skip.

# Acceptance Criteria

- A Muse screen capture (sanitised) holding an agentctl-typed draft,
  with background-panel rows under the editor, is flushed by the sweep.
- A Claude capture with a background-agent status footer, and one tall
  draft with no top rule visible, are recognised and flushed.
- An unparseable pane with a stranded queue prompt yields an
  unreadable finding and one blocked notice, never silence.
- A delivered (not quarantined) Claude draft is reported unattributed,
  not flush-attempted.
- No wall-clock budgets in the new tests (poll counts / fake clocks).

# Notes

Live deployment (services refresh/turnover) is the coordinator's job
after landing; the lane never restarts the watcher or bridges.

## Resolution

Landed 4cbc07e4 (gated 49e4cf3d, gate pass 2026-11-14T21:10:07Z). The
sweep reads Muse drafts through the Muse editor reader, attributes
under the flush's own vouching, reports unparseable panes with stranded
prompts as draft:unreadable findings, and the Claude view recognises
tall drafts whose top rule is off screen. Live on the host watcher and
bridges since the services turnover to 4cbc07e4 (2026-11-14 4:17 PM ET,
all units healthy).
