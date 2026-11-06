---
title: 'merge-queue-stale-green-stack: a candidate green on the previous base stacks even below the pass-rate gate when disjoint'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-06T13:09:01.430317101+00:00
updated_at: 2026-11-06T13:09:01.430317101+00:00
---

# Description

A consuming queue: below stacking gate every candidate validates as disjoint singleton root; each landing makes other in-flight greens stale and they rebuild as disjoint roots again; one change green on 3 bases without landing, about 4 of 24 validations stale-root rebuilds.

# Design

On existing P1 predictor and k-roots/keep_stale_roots (#915/c0c52444), not a parallel path: candidate GREEN on immediately previous base, stale only because unrelated changes landed, merge-tree clean against new base, no shared paths with what landed since its green, counts high-confidence for stacking: stack on current stack head even in all-roots/below-gate mode. Global pass-rate gate still governs fresh candidates. Record stale-green-stacked event (candidate, previous green base, landed changes checked against). Red stacked validation falls back to today's behaviour, no extra strike.

# Acceptance Criteria

Stacked when green-on-previous-base and disjoint; not stacked when paths overlap or merge-tree conflicts; red fallback; gate still applies to fresh candidates.
