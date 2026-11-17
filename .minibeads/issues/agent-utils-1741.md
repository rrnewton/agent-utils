---
title: 'wrkslots-audit-deletable-vs-remove-private-history: audit DELETABLE disagrees with remove on nested submodule private history'
status: closed
priority: 0
issue_type: bug
assignee: mu-removegaps
labels:
- wrkslots
created_at: 2026-11-16T16:30:20-05:00
updated_at: 2026-11-16T19:30:00-05:00
closed_at: 2026-11-16T19:30:00-05:00
---

# Description

[watermelon-full-preview] Field report: audit reports DELETABLE for agent slots whose nested submodule clones hold private history that remove then refuses (one slot: 10 commits in nested submodule clones); audit verdict and remove preconditions disagree. The slot lingers because the operator trusts DELETABLE, runs remove, and only then learns the history would be deleted. Keep consumer names out of this file.

# Design

- Make audit's DELETABLE verdict use the same private-history evidence remove uses, including nested submodule clones reached recursively below each checkout.
- When that evidence shows unpushed private history with kept tips, audit reports BLOCKED with remove's refusal text, which names the `--archive-private-history` remedy, instead of DELETABLE.
- Python and Rust parity where audit exists in both editions.
- No fetch policy change beyond the private-history evidence remove already gathers; a scan failure blocks, never clears.

# Acceptance Criteria

- A test builds a slot with private history only in a nested submodule and asserts audit and remove agree: audit is BLOCKED naming the private history and the `--archive-private-history` remedy, and remove refuses with the same history.
- A slot whose nested submodule history is fully durable stays DELETABLE in both editions.
- Existing audit and remove suites stay green.

# Notes

High priority: the owner ordered a host-wide slot cleanup today, and lingering validate checkouts motivated the census behind this report.

## Resolution

[watermelon-full-preview] Landed as 140b440d on main. Audit now runs remove's private-history scan over every initialized submodule, nested clones included, in both editions: a kept private tip blocks with remove's own refusal naming --archive-private-history, so DELETABLE no longer names a slot remove would refuse. Regression tests build a slot with private history only in a nested submodule and assert audit and remove agree, in Python and in the Rust differential. Validated and gated on the exact tree, with the combined tree revalidated for the overlap landing.
