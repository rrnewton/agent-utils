---
title: 'agentctl-muse-resume-readiness: start --resume and revive of a Muse lane fail readiness although Muse resumed to an idle composer'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T18:16:13.000000000+00:00
updated_at: 2026-11-06T18:16:13.000000000+00:00
---

# Description

After a host reboot, `agentctl start --resume SESSION` and `agentctl revive` of Muse lanes ended with "did not reach a verified idle composer" or "revive incomplete ... candidate cannot be verified". Each Muse had in fact resumed cleanly and showed an idle composer.

The resumed screen carries extra lines that a fresh start does not:

- "warning: the previous run of this session ended abnormally ... It was resumed cleanly"
- for a profile that asks for ultra reasoning: "reasoning effort ultra is not available ... using xhigh"

The readiness matcher appears to reject a screen with these lines, so a successful resume is reported as a failure, and the operator has to verify and repair each lane by hand.

# Design

- Capture the real resumed screens (both banners) as fixtures.
- Make Muse readiness recognise the idle composer underneath resume and effort-fallback banners. Keep the stranded-prompt and approval-prompt refusals as they are.
- Report a downgraded reasoning effort in the start/revive result instead of treating it as a failure.

# Acceptance Criteria

- Tests with both banners: start --resume and revive reach a verified idle composer and exit 0.
- A real failure (no composer, a prompt still staged) is still refused.

# Notes

Reported 2026-11-06 by the lead coordinator during a reboot recovery. Related: #947 agentctl-revive-candidate-unpinnable, #948 agentctl-muse-native-session-missing, #949 restore-muse-lanes.
