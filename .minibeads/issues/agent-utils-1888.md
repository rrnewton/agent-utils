---
title: 'agentctl-muse-placeholder-hint-reads-as-staged: Muse placeholder hints read as unsent composer text hold deliveries'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-18T00:00:00.000000000+00:00
updated_at: 2026-11-18T00:00:00.000000000+00:00
---

# Description

Muse draws rotating placeholder hints inside an empty composer (for example "/goal pins a session objective with a progress bar", "Ask to monitor a running build or log and get pinged on changes"). agentctl intermittently reads such a composer as holding unsent text: the pane reports agent_status "staged" and every delivery is held (drain: "did not become idle/done ... last status=staged"), sometimes for hours, until a person types into the pane. Seen live on Muse lanes of two teams on 2026-11-18, including sends from a coordinator that timed out all evening. #1773 agentctl-muse-placeholder-hint-as-draft (closed) taught the editor readers the exact, wrapped, edge-truncated and ellipsis-truncated hint shapes; the misses that remain are hint text captured short of the whole ghost — a partially drawn or clipped hint that neither fills its row to the pane edge nor ends in an ellipsis, which the readers still fail closed as a draft. Same class as #1500 codex-idle-animation-reads-as-draft (closed).

# Design

In both editions, a composer whose whole text is a long prefix of exactly one known Muse placeholder hint reads empty, even when the prefix reaches neither the pane edge nor an ellipsis: at least 34 characters covering at least two-thirds of the hint it prefixes. Shorter prefixes, and any text that is not a hint prefix, still read as a draft and fail closed exactly as today, so a person typing (including someone who starts typing a hint's opening words and stops early) is never typed over. The known-hint fixtures cover every hint in the versioned table, whole, in both editions; the hint set itself is unchanged (the installed Muse's set is the same 23).

Recovery: prompts already held by this are never typed or quarantined — they wait pending in the inbox (readiness refused before any typing), so once a build with the fix is the one draining, the next drain delivers each exactly once with no duplicate and no action on the lane.

# Acceptance Criteria

- Every hint in the versioned table, drawn whole in an empty Muse composer, reads as an empty composer (idle when no turn runs, working not staged when one does) in both editions.
- A long bare prefix of a hint (at least 34 chars, at least two-thirds of its hint, reaching neither edge nor ellipsis) reads empty in both editions.
- A short hint prefix, a hint plus extra words, and ordinary draft text still read staged in both editions.
- Focused suites pass in both editions; the fix is exercised through the real CLI against a test Muse agent before landing.

# Notes

Reported from live lanes on two teams on 2026-11-18 (placeholder examples quoted in the report match table entries; the failing captures were transient mid-draw frames). No lane, host or person is named here.
