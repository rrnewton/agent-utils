---
title: 'mb-id-collision-guard: stop two lanes from filing different issues under the same number'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-06T18:34:21.000000000+00:00
updated_at: 2026-11-07T03:24:19.000000000+00:00
closed_at: 2026-11-07T03:24:19.000000000+00:00
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

Status from mu-validate, 2026-11-06: the landing guard ships in this change
as `scripts/check_issue_numbers.py` (wired into validation/repository.dag.yaml
as repository.hygiene.issue-numbers plus its self-test, and `make
check-issue-numbers`). It refuses a touched issue file whose number the target
main already files under a different slug, naming both slugs, and lets edits
under the existing slug through; a change filing one new number twice is
refused the same way. Chosen base handling: the target main is `--base` or the
first of github/main/origin/main that resolves, fail-closed when none does.

Atomic allocation choice: reserve the number against the canonical remote with
compare-and-swap (a pushed reservation object or lease-guarded ref update),
NOT collision-proof suffixes — the readable `#N slug` convention (#86-era
agents cite numbers in commits, PRs and chat, and renumbering breaks those
quotes) is worth the guard. `mb` belongs to the external minibeads tool
consumed via ext/minibeads + PATH, so the implementation is filed as #991
mb-create-atomic-allocation instead of being written here.

One follow-up seen while wiring: two lanes pushing the same stale issue-edit
(the same slug edited differently) still needs the normal content rebase; this
guard covers the number/slug collision it was filed for.

## Resolution

Landed on main as 78056f94 (with the #932 close as b593bc79). The guard
refused three of its own follow-up filings during development as main took
the numbers first, which was its first live exercise. Atomic allocation for
`mb create` is filed as #991 mb-create-atomic-allocation. A companion hygiene
step parsing every issue file with the pinned `mb` reader follows separately,
so a file mb refuses fails the gate instead of blocking `mb` repo-wide
unnoticed.
