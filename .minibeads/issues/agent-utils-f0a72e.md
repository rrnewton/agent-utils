---
title: 'sent-receipt-deadline: recheck an early expiry timer before the grace fallback'
status: closed
priority: 2
issue_type: bug
assignee: codex-vibe-voice
created_at: 2026-10-27T18:10:16.970372052+00:00
updated_at: 2026-10-27T19:00:30.880498844+00:00
closed_at: 2026-10-27T19:00:30.880498779+00:00
---

# Description

[GPT-6 Codex] Fixed a pre-existing early-timer defect: a callback one millisecond before the 15-second deadline kept a visible receipt but postponed its next check to the ten-minute fallback. The deterministic regression loaded against pristine main’s actual web assets fails at that behavior. The fix retries the original deadline, with at least a one-millisecond timer, then retires the receipt at the deadline.

The discriminating regression, original real-browser sent-receipts walk and complete selected validator pass. The showing interval and timeout budgets were not changed. Historical related record: #500 sent-receipts-pile-up. Delivery: agent-utils-f0a72c vibe-voice-validation.

Published code: 085a64cc6eb8e10618acbfbd7bee915b7c7b9fd2. Complete selected make validate: 123 steps passed, zero failed at 76eb9dbb (identical entire application tree); after rebase, 31 canonical voice/pairing/Rust/JavaScript/hygiene checks passed on the published head.
