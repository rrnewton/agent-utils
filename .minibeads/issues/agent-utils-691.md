---
title: 'muse-bridge-herdr-manifest: Herdr screen rules for Muse, or keep agentctl the only reader'
status: open
priority: 3
issue_type: task
created_at: 2026-10-29T15:54:23.499703403+00:00
updated_at: 2026-10-29T15:54:23.499703403+00:00
---

# Description

[opus 5.5] From #679 muse-bridge-study. Herdr has no screen manifest for Muse (`herdr agent
explain --file SCREEN --agent muse` prints `manifest: none`, `fallback_reason: unknown_agent`),
so Herdr's own status for a Muse pane is coarse: it reports `done` once the answer is drawn,
during Muse's post-turn "Double checking" phase (60-120 s at xhigh). agentctl no longer depends
on it (#678 computes Muse status from the screen), but other Herdr consumers do.

Proposed rules, if Herdr takes local or upstream manifests:

- working: a row at column 0 matching `^[◆◇◈] .*\(.*esc to interrupt\)$` among the 16 non-empty
  rows above the editor's two rules;
- blocked: `Would you like to run the following command?` or `Allow this stage once` in the
  bottom 24 rows with no editor drawn;
- idle: the editor (`❯` between two rules, one `·`-separated footer) with no such row.

Decide whether to upstream this to Herdr or keep agentctl as the only Muse reader.
