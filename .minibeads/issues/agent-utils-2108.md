---
title: 'orc-subteam-trial-plan: plan an Orc-run subteam trial and a later team migration'
status: open
priority: 2
issue_type: task
labels:
- coordinator
created_at: 2026-11-22T10:10:00-05:00
updated_at: 2026-11-22T10:10:00-05:00
---

# Description

The owner wants a subteam run by Orc (its own tmux, coordinator in the first tab, minimal TUI preferred) to compare against agent-utils coordination over time. First candidate: the lightly used agent-utils team on one host; later, with another consumer team, that team on another host. Fresh, globally numbered task databases from here on.

# Design

A plan document first: tool install and checks (owner-approved tools only; no system packages without the owner), task database naming, the chat bridge experiment (our bridge to the minimal TUI through the tmux backend, keeping its web UI, before any Slack setup of its own), agent-teams messaging integration (allow-listing the Orc coordinator), house rules mapped onto Orc lanes (slots, bot identity, gates and land-main, no installers), restore and turnover gaps, transcript ingestion for comparison, model quota and gateway, adoption tested on a throwaway session, and rollback to the current coordinator. No changes on the candidate host until the owner sees the plan.

# Acceptance Criteria

- The plan is published and linked; the owner has reviewed it before any host change.

# Notes

Filed by the coordinator; the owning lane refines this file.
