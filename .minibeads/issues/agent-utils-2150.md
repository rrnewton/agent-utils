---
title: 'agentctl-watch-restore-report-false-alarm: agentctl watch reports a missing restore report with no outage'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-23T06:15:00-05:00
updated_at: 2026-11-23T06:15:00-05:00
---

# Description

Since the morning turnover, agentctl watch reports 'restore <id>: no completed restore report for this boot' on two hosts where nothing went down (one host booted three weeks ago). The check seems to expect a restore report for every boot or every refreshed restore unit, not only after a restore actually ran.

# Design

Alarm only when a restore was expected and did not complete: for example the restore unit ran this boot and has no completed report, or the team was found down. A turnover that refreshed the restore unit without a reboot must not count as a restore this boot. Say in the message what was expected and why.

# Acceptance Criteria

- Test: a host that booted long ago, with a refreshed restore unit and no restore run, produces no alarm; a restore that started this boot and never completed still alarms.

# Notes

Reported by a consuming team after the morning services turnover; filed by the coordinator.
