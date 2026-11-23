---
title: 'chat-bridge-owner-rescan-starved-by-host-budget: a bridge owner rescan held back by the host listing budget for over a day'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-11-22T22:50:00-05:00
updated_at: 2026-11-22T22:50:00-05:00
---

# Description

On a host running several chat bridges, one bridge's chat health reads "DEGRADED: the owner rescan has been held back by the host listing budget for 29.5 h (last hold 3.2 min ago)". The owner rescan exists to pick up owner messages the live event stream never carries (for example file messages), so a rescan starved for a day can miss owner messages for that long. The condition was only surfaced when the bridge moved to a newer build; the older build reported OK while the hold was in force.

# Design

- Find why the shared per-host listing budget never lets this bridge rescan: for example one bridge or channel consuming the whole budget each window, a budget sized for one bridge shared by several, or a hold that never resets.
- Make the budget fair across bridges (each bridge gets its turn within a bounded time), and report any hold longer than a few rescan intervals as a health warning with the bridge that holds the budget.
- Keep the change constant-time per pass; no unbounded listing.

# Acceptance Criteria

- With several bridges sharing one host budget, every bridge completes an owner rescan within a bounded interval (a test with a simulated budget and three bridges).
- Health reports which bridge or channel is consuming the budget when a hold persists.

# Notes

Reported by a consuming team after a bridge refresh; filed by the coordinator.
