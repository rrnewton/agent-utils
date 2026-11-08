---
title: 'frozen-incomplete-operator-release: no operator path releases an incomplete retained frozen validation checkout'
status: open
priority: 3
issue_type: feature
created_at: 2026-11-08T20:45:00.000000000+00:00
updated_at: 2026-11-08T20:45:00.000000000+00:00
---

# Description

Follow-up to #1088 frozen-reclaimable-build-output. `recover-ownerless-validate-batch` (through `_durable_frozen_result`) removes a frozen validation checkout only when there is a completed terminal service result and a matching `frozen/<unit>.jsonl` row. By design it refuses incomplete records. So a checkout whose run ended COULD_NOT_RUN with no result is retained forever: there is no operator path to release it.

A consumer's devbig014 host holds one such checkout, about 47.7 GB (state schema 8, COULD_NOT_RUN, no result). The missing result came from a since-fixed parser defect, so the checkout holds no evidence anyone needs.

# Design

An operator command that releases ONE named incomplete retained checkout:

- a mandatory `--reason`;
- a liveness check: no live run, process, mount or open file uses the checkout;
- the usual salvage of any git state not on a remote (bundle under the salvage area);
- an audit record: an operation-log event, plus a durable note naming who, why, what was removed and the salvage location;
- refusal for any checkout that has a result (the normal recovery path owns those) or any live reference;
- a dry run by default, or a printed plan before acting, consistent with other destructive wrkslots commands.

Python first. Mirror in Rust, or record the parity gap for the Rust wrkslots owner.

# Acceptance Criteria

- An incomplete retained checkout with no live reference is released after salvage, with an event and a note.
- A checkout with a result, a live process, mount or open file, or a missing `--reason` is refused with a clear remedy.
- Tests cover each refusal and the success path, including salvage of an unpushed commit.
- The command is documented in the CLI help and the user guide.

# Notes

Requested by the lead (devbig014) on 2026-11-08 on a consumer's behalf. P3. It can follow #1088 in the same lane.
