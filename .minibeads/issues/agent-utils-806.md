---
title: 'relay-in-ignore-own-host: relay-in ignores copies its own host posted before key checks'
status: open
priority: 2
issue_type: bug
labels:
- agent-teams
created_at: 2026-11-03T02:06:36.894337228+00:00
updated_at: 2026-11-03T02:06:36.894337228+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator. Once two teams on one host are both relay-enabled, a message between them is mirrored into the shared relay channel, and each host's relay-in then reads its own host's mirror copies. relay-in checks every copy's signature against its trusted keys first, so a host that has not trusted its own teams' keys raises stranger alarms and quarantines its own posts. On 2026-11-02 this tripped stranger alarms on one host the moment a second team's relay_channels entry was added, and forced a rollback. Today's workaround is to `relay trust-key` every local team@host on its own host.

# Design

relay-in recognises copies that this host itself posted (the envelope's from team@host is a local team of this host and the post's ID is in this host's own outbox or mirror record) and ignores them before any key or sender check: no admission, no quarantine, no alarm, and a team-log event saying so. A post that claims a local team@host but is NOT in this host's own records is still checked and alarmed as today, because that would be an impersonation.

# Acceptance Criteria

- With two relay-enabled teams on one host and neither local key trusted, a message between them produces its direct delivery and no quarantine or alarm on that host.
- A forged post claiming a local team@host, absent from this host's outbox, is still quarantined with an alarm.
- Tests cover both cases.
