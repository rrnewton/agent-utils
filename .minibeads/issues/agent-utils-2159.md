---
title: 'gate-shared-host-admission-declaration: gate.py admits heavy gates against a host-wide disk and memory declaration shared with other teams'
status: open
priority: 2
issue_type: feature
labels:
- gate
created_at: 2026-11-23T08:05:00-05:00
updated_at: 2026-11-23T08:05:00-05:00
---

# Description

On a host shared with another team's heavy validations, our gates are admitted only against our own slot cap; the other team's dispatcher keeps its own disk and memory floors, and neither side sees the other's planned peaks. Last night a coordinator had to throttle and freeze gates by hand (and with a watcher script) to keep a shared disk above the other team's admission and stop lines.

# Design

- gate.py reads a host-wide admission declaration (a small file in a host state directory both teams can read): disk and memory floors (pause line, hard stop) and the other team's declared in-flight peaks.
- Each gate declares its expected peak (learned from past runs of the same selection) and is admitted only if the floors still hold after that peak; otherwise it waits, logged, with the reason.
- Running gates are not killed; below the hard-stop line they are frozen (cgroup freeze) and thawed above the pause line, every change logged.
- The declaration format is versioned and documented so another team's dispatcher can write its own entries.

# Acceptance Criteria

- Tests with a fake disk reading: a gate waits when its declared peak would cross the pause line, starts when room returns, and a running gate is frozen and thawed at the stop and pause lines; all decisions logged.

# Notes

Requested by a consuming team sharing the host; filed by the coordinator.
