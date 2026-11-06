---
title: 'mb-id-collision-guard: stop two lanes from filing different issues under the same number'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-06T18:34:21.000000000+00:00
updated_at: 2026-11-06T18:34:21.000000000+00:00
---

# Description

On 2026-11-06, five times, two lanes filed different issues under the same number. Each lane takes the next free number from its own stale view of main, and the collision shows up only at landing, or later as a silently replaced file.

# Design

Two parts. Either alone helps.

- **Atomic allocation:** `mb create` reserves its number against a shared source, not the local checkout's highest number. Options: a reservation ref pushed with compare-and-swap, or numbers that cannot collide, such as a per-lane prefix or a short random suffix kept beside a display number. Weigh how much the readable `#N slug` convention matters.
- **Landing guard:** a check refuses a change that adds `.minibeads/issues/<prefix>-N.md` when that path already exists on the target main with a different slug. It runs in the repository hygiene step, against the base, and is cheap enough for every gate. Landing tools may run it too. An edit to an existing issue, with the same slug, stays allowed.

# Acceptance Criteria

- A test: adding issue N with slug A when main already has N with slug B is refused, and the message names both slugs.
- A test: editing issue N with its existing slug passes.
- The allocation choice is written up, and either implemented or filed as a follow-up.

# Notes

Requested 2026-11-06 by the lead coordinator after five collisions in one day.
