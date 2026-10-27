---
title: 'browser-audio-output: isolate headless checks from unavailable host audio'
status: closed
priority: 1
issue_type: bug
assignee: codex-vibe-voice
created_at: 2026-10-27T18:10:16.956753038+00:00
updated_at: 2026-10-27T19:00:30.870215093+00:00
closed_at: 2026-10-27T19:00:30.870215028+00:00
---

# Description

[GPT-6 Codex] Repaired headless browser audio fixtures with Chromium’s final-OS-output test sink. Decoding, audio clocks, scheduling, latency budgets and slow controls remain active. Four original audio steps failed on pristine main 1f0a7a90 under one step/two CPUs/4 GB/nice 15/idle I/O. Changing only the test process’s unavailable PULSE_SERVER socket made all four unchanged main checks pass. The portable shared fixture then passed all four on the branch without that override in 119.6 seconds. The earlier clock-assertion adjustment was removed.

The complete selected validator covers the shipped repair. This proves browser behavior and preserves negative controls; physical-speaker acceptance remains with the owner after redeployment. No deployment audio environment changed. Historical related record: #234 oss-redeploy-check. Delivery: agent-utils-f0a72c vibe-voice-validation.

Published code: 085a64cc6eb8e10618acbfbd7bee915b7c7b9fd2. Complete selected make validate: 123 steps passed, zero failed at 76eb9dbb (identical entire application tree); after rebase, 31 canonical voice/pairing/Rust/JavaScript/hygiene checks passed on the published head.
