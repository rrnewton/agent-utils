---
title: 'frozen-incomplete-operator-release: no operator path releases an incomplete retained frozen validation checkout'
status: closed
priority: 3
issue_type: feature
created_at: 2026-11-08T20:45:00.000000000+00:00
updated_at: 2026-11-08T21:45:47.000000000+00:00
closed_at: 2026-11-08T21:45:47.000000000+00:00
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

## Resolution

Delivered by cl-reclaim as `wrkslots release-frozen-incomplete`.

- Arguments: `--frozen-validate-checkout`, `--completed-record`, `--repository`, a mandatory non-blank `--reason` (at most 2000 characters), `--coordinator-authorized`, `--coordinator-pid` and `--format`. Without `--yes` it is a dry run: it prints the size, the run-record digest, and whether a remote holds each HEAD, and changes nothing.
- Authority: the #1088 retention proof, i.e. `_frozen_no_proof_disposition` must classify the checkout as current-incomplete-retained (authenticated incomplete record, dead process, pristine clone and submodules, shared plus fresh census, mountinfo). A recorded result refuses with "recover-ownerless-validate-batch removes it". A live process, open file or mount, a live run generation, a dirty clone, any other record shape, or a blank reason also refuses.
- Salvage: the clone HEAD is checked on the configured remote through the source repository (as removal does); each initialized submodule HEAD is checked in its own remote-tracking refs. Every HEAD not held is bundled, verified and receipted with the existing local-salvage code under `<control>/wrkslots-salvage/`, before the journal.
- Removal: the ordinary `ownerless-validate-remove` journal, with a new `operator_release` terminal-evidence kind (release id, reason, operator user/uid/coordinator PID, note path, salvage list). Resume re-verifies the incomplete run-record shape and every bundle digest, and when the clone HEAD was salvaged it does not require remote containment of it. The `ownerless-validate-path-removed` event therefore carries the reason and operator.
- Note: `<control>/wrkslots-salvage/frozen-releases/<id>.md`, written before the journal and updated with the outcome. This is under the salvage area rather than `<control>/frozen-releases/`: in the nested layout the control directory is the slots directory, and every edition (and older builds) already treats `wrkslots-salvage` as storage, whereas a new top-level directory would read as an unregistered slot.
- Tests (`py/wrkslots/tests/test_lifecycle.py`, `test_release_frozen_incomplete_*`): a dry run changes nothing; success with a pushed target (no salvage) and an unpushed one (bundle restored and checked); submodules held by their remotes; refusals (blank reason, live process, open file, mount, dirty clone, live generation, a non-incomplete record, a recorded result, a missing `--reason`); resume after the journal and after removal; a tampered bundle refuses the resume and keeps the checkout.
- Documented in CLI help and in the user guide, "Release one incomplete retained frozen checkout".
- Rust parity gap, for the Rust wrkslots owner: Rust implements neither frozen batch classification nor ownerless validation removal or its resume (`classify_ownerless_validate_batch` and `recover_main_write.rs` both refuse with "stays with the Python edition"). The soak router sends `release-frozen-incomplete` to Python as an unknown command. A Rust port of ownerless validation removal must accept and re-verify the `operator_release` evidence kind; Rust journal shape checks already treat evidence as an opaque mapping.

