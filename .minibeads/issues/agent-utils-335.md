---
title: 'offline-cache-chips-flake: the offline-cache check sees Collapse all over Send under load'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T15:52:38.638065192+00:00
updated_at: 2026-10-19T10:29:57.207861117+00:00
closed_at: 2026-10-19T10:29:57.207860987+00:00
---

# Description

Found in the final review of #269 approval-card-inline (2026-10-15): vibe-talk.check.offline-cache-browser fails now and then under heavy host load with 'desktop, the thread composer under the floating chips: ["Collapse all" covers #channel-send]', on main as well as on the branch (22 solo reruns passed). Find whether the chips really cover the send button for a frame while the layout settles (then fix the page), or whether the check measures too early (then wait for layout to settle). Prove it under load as #314 reply-focus-flake did.
