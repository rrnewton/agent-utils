---
title: 'relay-in-ignore-own-host: relay-in ignores copies its own host posted before key checks'
status: closed
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agent-teams
created_at: 2026-11-03T02:06:36.894337228+00:00
updated_at: 2026-11-03T03:31:46.403010112+00:00
closed_at: 2026-11-03T03:31:46.403009711+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator. Once two teams on one host are both relay-enabled, a message between them is mirrored into the shared relay channel, and each host's relay-in then reads its own host's mirror copies. relay-in checks every copy's signature against its trusted keys first, so a host that has not trusted its own teams' keys raises stranger alarms and quarantines its own posts. On 2026-11-02 this tripped stranger alarms on one host the moment a second team's relay_channels entry was added, and forced a rollback. Today's workaround is to `relay trust-key` every local team@host on its own host.

# Design

[opus 5.5] (Revised with the coordinator 2026-11-03: admit on the local key, not ignore; on one host the relay copy can be the only path, as in relay_e2e "signed both ways".) relay-in admits a signed envelope whose from.host is THIS host when its signature verifies against the signing key that team@host keeps on this host (relay/keys). It is logged `own-host` and takes the normal ledger path: `duplicate-known` (outcome `known`) when the direct delivery already recorded the ID, as after a mirror, and delivered otherwise. It is never a stranger, and needs no trust-key and no listed-poster check. A signed envelope claiming this host with no local key for that team, or whose signature fails against it, is quarantined with an alarm (an impersonation). Unsigned (legacy one-bot-per-team) envelopes are unchanged.

Same-host cross-team traffic stays mirrored: the owner wants it visible in the relay channel. The `agent-teams relay trust-key TEAM@THISHOST` workaround (applied on both hosts on 2026-11-02) is no longer needed, and harmless.

# Acceptance Criteria

- With two relay-enabled teams on one host and neither local key trusted, a message between them produces its direct delivery, an `ignored-own-host` or `duplicate-known` relay-in record, and no quarantine or alarm on that host.
- An envelope claiming this host whose signature fails against the local key is still quarantined with an alarm.
- Tests cover both cases.

# Notes

## Resolution

[opus 5.5] relay-in admits a signed envelope from another team on this host on the key that team keeps here (relay/keys): logged `own-host`, then the ledger path (`known`, quiet `duplicate-known`, after the direct delivery; delivered when the relay copy is the only one). Never a stranger; no trust-key or listed-poster check. A signed claim of this host that no local key verifies is quarantined with an alarm. Unsigned envelopes unchanged. Test: relay_e2e own_host_mirror_copy_is_admitted_on_the_local_key_without_trust (two local teams, no trusted keys: direct delivery, mirror copy `known`, no quarantine or alarm; a copy signed elsewhere is quarantined with an alarm). The two tests that modelled an untrusted remote team with a key kept on this host now set that key aside while the team is a stranger. "signed both ways" passes unchanged. The trust-key workaround on both hosts is no longer needed, and harmless.
