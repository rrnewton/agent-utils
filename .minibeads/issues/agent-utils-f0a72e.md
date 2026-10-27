---
title: 'sent-receipt-deadline: recheck an early expiry timer before the grace fallback'
status: in_progress
priority: 2
issue_type: bug
assignee: codex-vibe-voice
created_at: 2026-10-27T18:10:16.970372052+00:00
updated_at: 2026-10-27T18:10:16.978660224+00:00
---

# Description

[GPT-6 Codex] Confirmed on pristine main 1f0a7a902278276ca3a76badfd26f8c2af29ea61 using a deterministic regression case loaded against its unmodified web assets. armSentReceipt schedules the 15-second callback; if that callback fires one millisecond before Date.now reaches the deadline, sentReceiptRetires correctly keeps the receipt, but armSentReceipt then schedules its next check at the ten-minute grace. The receipt remains for ten minutes. Related prior fix: #500 sent-receipts-pile-up.

Original boxed sent-receipts-browser passed once on main but failed on the feature branch under identical one-step/two-CPU/4-GB limits. A new one-millisecond-early control fails on the actual clean-main script with “the early timer postponed its next check to the ten-minute grace”, establishing that the timer defect predates the branch. The fixed branch re-arms the same deadline when early and passes this control without shortening the receipt's showing or raising any timeout.

Acceptance: early callback preserves the receipt until its deadline, rechecks after the remaining millisecond, and retires it then. Existing real-browser sent-receipts walk and the selected gate remain required.
