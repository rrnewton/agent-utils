---
title: 'voice-approval-browser: stabilize playback readiness in the boxed call walk'
status: closed
priority: 2
issue_type: bug
assignee: codex-vibe-voice
created_at: 2026-10-27T18:12:27.831038361+00:00
updated_at: 2026-10-27T19:00:30.890693454+00:00
closed_at: 2026-10-27T19:00:30.890693389+00:00
---

# Description

[GPT-6 Codex] Repaired a pre-existing browser-fixture race. Clean main b19c1406, under two steps/eight CPUs/32 GB/nice 15/idle I/O, reproduced the correct refusal “the yes came before the read-back finished playing” in two of six instrumented call walks. Fixed sleeps did not establish the fixture’s intended playback precondition. No application approval defect was established.

The fixture now waits for numbered read-back completion and both playback clocks within the SAME 2.5-second budget. All six repaired call walks and six concurrent pause/reply/resume controls pass (12 checks, zero failures, 145.2 seconds), as does the complete selected validator. All approval, cancellation, echo and late-transcript assertions remain. Delivery: agent-utils-f0a72c vibe-voice-validation.

Published code: 085a64cc6eb8e10618acbfbd7bee915b7c7b9fd2. Complete selected make validate: 123 steps passed, zero failed at 76eb9dbb (identical entire application tree); after rebase, 31 canonical voice/pairing/Rust/JavaScript/hygiene checks passed on the published head.
