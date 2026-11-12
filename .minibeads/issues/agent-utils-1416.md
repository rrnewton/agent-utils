---
title: 'team-restore-verify: a restore is done only when health checks pass, including bridge targets and blocked panes'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] On 2026-11-12 a restore reported complete while: the team's chat bridge was restart-looping because its target agent was missing, then stopped on a stream gap; two revived Claude panes sat behind an unrecognised "Teach auto mode about your environment?" dialog for about 7 minutes with owner messages queued behind them; six lanes ran at the wrong effort (#1412); and queued messages had been archived (#1413). See section 5 of the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

# Design

VERIFY (a state of #1414, usable alone as `agentctl team verify T`) passes only when:
1. every desired agent is live in Herdr with its declared profile, effort and launch environment;
2. every unit is active, and each chat bridge reports `chat health` OK with its target agent present;
3. no queued message was archived undelivered (#1413);
4. the watcher runs and sees no unexplained dead agents (no non-agent directory under agents/, #1359);
5. every revived pane is at an input prompt: known benign dialogs are cleared from an explicit allowlist (this "Teach auto mode" dialog is the first entry), and anything else is a failed check handed off;
6. every coordinator acknowledged the outage notice, or its timeout expired.
Each failed check names the agent or unit, the evidence, and the fix the core skill gives.

# Acceptance Criteria

- Fixture tests per check, including a bridge whose target is missing and a pane behind the dialog.
- `team verify` is read-only apart from the allowlisted dialog clears, and its JSON names every check.

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).
