---
title: 'spawn-new-slot: create the slot and its agent together and record the pairing'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-11T14:48:05.744323716+00:00
updated_at: 2026-11-11T14:48:14.580516795+00:00
---

# Description

Owner design point (Slack 6231, 2026-11-11): spawning belongs in the tools. A consuming project's spawn wrappers (33 and 28 lines) each reimplement the same sequence: refuse harness-repo slots without an explicit delicate flag, create the slot with per-repo branches, start the agent in the slot, send the brief with setup rules (adopt the slot, write only in the slot, keep it slim), and set the goal. Because the sequence lives in wrappers, policy (which repos may be slotted, what the brief must say) drifts per caller, the slot-to-agent pairing is recorded nowhere the tools own, and a failure midway leaves a slot with no agent or an agent with no slot for the next operator to untangle.

# Design

Add one spawn operation that creates the slot and the agent together and records the pairing: agentctl start --new-slot <repos> (or the slot-tool-side equivalent; name settled in implementation). It runs the existing create with per-repo branches, starts the agent in the created slot through the existing start path (profile, slot isolation, slot project recorded as today), delivers the brief, and sets the goal when given -- as one journalled operation: a failure after slot creation either completes the start on retry with the same slot or reports the exact half (slot without agent, with its recovery command), never a second slot for the same spawn. The pairing (agent name, slot, generation, repos/branches) is recorded where both tools already read ownership, so status on either side names the other. Harness-repo refusal and delicate opt-in come from the harness-layout declaration, not wrapper policy: spawn refuses a harness-repo slot exactly as create does. Existing separate create and start commands keep their behaviour; spawn is composition, not a replacement path with different safety checks.

# Acceptance Criteria

- One spawn invocation creates the slot (per-repo branches as given), starts the named agent in it, delivers the brief, and records the pairing readable from both agent status and slot status; no second slot or worktree is created.
- Killing spawn between slot creation and agent start, then re-running the same spawn, completes against the same slot instead of creating another or refusing on its own residue; the half-state report names the recovery command when it cannot complete.
- Spawn naming the harness repo refuses under the same rule and flag as create (harness-layout declaration), with no slot created.
- The brief delivered by spawn is the caller's file verbatim plus the recorded setup rules the tools already enforce (adopt/ownership is already bound by construction and the brief says so instead of instructing a manual adopt).
- Separate create-then-start flows are unchanged; a fake-harness test proves spawn end to end including the interrupted-retry case in both editions.

# Notes

Owner design point Slack 6231 (2026-11-11), part B1 of three: B2 is the review-seat issue filed alongside this one, B3 is the harness-layout-mode refusal. Issue-only here; the wrapper sequences it replaces stay working until spawn lands.
