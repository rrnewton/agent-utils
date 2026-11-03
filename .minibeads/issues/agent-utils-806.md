---
title: 'relay-in-ignore-own-host: relay-in ignores copies its own host posted before key checks'
status: open
priority: 1
assignee: cl-start@devbig030
issue_type: bug
labels:
- agent-teams
created_at: 2026-11-03T02:06:36.894337228+00:00
updated_at: 2026-11-03T02:06:36.894337228+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator. Once two teams on one host are both relay-enabled, a message between them is mirrored into the shared relay channel, and each host's relay-in then reads its own host's mirror copies. relay-in checks every copy's signature against its trusted keys first, so a host that has not trusted its own teams' keys raises stranger alarms and quarantines its own posts. On 2026-11-02 this tripped stranger alarms on one host the moment a second team's relay_channels entry was added, and forced a rollback. Today's workaround is to `relay trust-key` every local team@host on its own host.

# Design

relay-in treats an envelope whose from.host is THIS host as its own copy: it verifies the signature against the local signing key for that team@host (relay/keys), then records `ignored-own-host`, or `duplicate-known` when the ledger already has the ID. It never treats such a copy as a stranger. An own-host envelope whose signature does not verify against the local key is still quarantined with an alarm, because that would be an impersonation.

Same-host cross-team traffic stays mirrored: the owner wants it visible in the relay channel. Until this lands, the workaround is `agent-teams relay trust-key TEAM@THISHOST` for every local relay-enabled team (applied on both hosts on 2026-11-02).

# Acceptance Criteria

- With two relay-enabled teams on one host and neither local key trusted, a message between them produces its direct delivery, an `ignored-own-host` or `duplicate-known` relay-in record, and no quarantine or alarm on that host.
- An envelope claiming this host whose signature fails against the local key is still quarantined with an alarm.
- Tests cover both cases.
