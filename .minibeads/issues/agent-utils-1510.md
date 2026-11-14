---
title: 'local-ci-hub-nested-canonical-digest: canonicalize nested evidence in typed struct order'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-14T02:42:14.072565420+00:00
updated_at: 2026-11-14T02:42:14.072565420+00:00
---

# Description

Consumer champion condition 2: 1 of 30 sampled digests genuinely diverges in nested canonicalization (coverage artifact plus gate attempts in one validate run record).

# Design

Canonicalize nested typed evidence in struct order at every depth in digest.rs; untyped shapes keep generic sorted form; synthetic unit + differential tests only.

# Acceptance Criteria

Synthetic nested records byte-equal to typed oracle form; untyped shapes keep sorted form; full 5709-digest comparison green on champion front door via sent SHA.
