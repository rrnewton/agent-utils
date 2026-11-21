---
title: 'agent-teams-ledger-receipt-reconcile: receipts must reflect the real queue outcome'
status: open
priority: 1
issue_type: bug
labels:
- agent-teams
created_at: 2026-11-21T02:50:00-05:00
updated_at: 2026-11-21T02:50:00-05:00
---

# Description

[watermelon-full-preview] Most ledger entries stayed accepted (handed to the deliverer) whatever happened in the queue. Receipts must reflect the real outcome: delivered, possibly_submitted, failed or stranded, reconciled on read and on queue transitions, following archived and re-designated queues.

# Acceptance Criteria

- Receipt, status, seen and serve reconcile before answering.
- Confirmed processed is delivered; attempted-but-unconfirmed is possibly_submitted; never-attempted or archived-only items are stranded; revive-proved never-typed failures are failed.

# Exit-code table (sender-visible)

[watermelon-full-preview] agent-teams status/receipt exit by the reconciled recipient state: delivered/journaled 0; accepted/deferred/claimed/typing/presented/undeliverable 75 (pending, retry); possibly_submitted/failed/stranded/lost/ambiguous 76 (needs a human); expired/superseded/retracted/refused/fenced 65 (definite refusal). Before this fix status exited 75 from the outbox even when the recipient was delivered; a sender reading rc 75 as success must change.
