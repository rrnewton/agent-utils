---
title: 'agentctl-rebind-queue-target: a #987 rebind leaves the queue bound to the old target, so every send is refused'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T17:40:00.000000000+00:00
updated_at: 2026-11-07T17:40:00.000000000+00:00
---

# Description

On 2026-11-07 a Muse lane's record pinned session A while its pane ran session B (`muse ... resume B`), so every `agentctl send` stayed pending with "custom harness \"muse\" is not the foreground process in pane". `agentctl anchor NAME --replace` refused ("the resumed session contradicts the recorded session; revive it instead"). The #987 rebind `agentctl anchor NAME --replace --session-id B --new-conversation` then succeeded: the record now names session B.

But the queue's `target.json` still says `{"kind": "pane", "pane_id": ...}`, while the record's target is now `{"kind": "session", "value": B}`. Every `agentctl send` and `agentctl drain` now refuses outright: "queue ... is bound to {kind: pane ...}, refusing different target {kind: session ...}". The lane went from pending-forever to refused, so it is still reachable only by typing into its pane. Two messages queued before the rebind, one of them from many hours earlier, sit in the inbox with stale content.

# Design

- A successful `anchor --replace --new-conversation` (and any other rebind that changes the target kind or value) re-binds the queue's target in the same locked step, keeping the queue's entries, and records the old target in the journal.
- Pending entries queued before the rebind are reported, with their age, by the rebind command. The operator chooses `--deliver-pending` or `--cancel-pending`, so stale instructions are never typed silently into the new session.
- `agentctl status` shows a mismatch between the record target and the queue binding as a named problem with the fix.

# Acceptance Criteria

- A test: after `anchor --replace --session-id NEW --new-conversation`, `agentctl send` delivers to the live session, and the queue binding names it.
- A test: pending entries from before the rebind are neither delivered nor dropped silently; the rebind lists them and acts only on an explicit flag.
- `agentctl status` names a record/queue target mismatch.

# Notes

Found by the devbig030 coordinator right after deploying agentctl 888124cc, while rebinding a long-running Muse lane under #987 agentctl-muse-fresh-rebind. The lane's pending queue was left in place, not hand-edited.
