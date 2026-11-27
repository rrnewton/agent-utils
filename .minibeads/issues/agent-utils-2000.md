---
title: 'wrkslots-lock-order: never hold the config lock while waiting for the ACTIVE global lock'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
- locking
created_at: 2026-11-20T14:43:46-05:00
updated_at: 2026-11-26T22:22:41-05:00
closed_at: 2026-11-26T22:22:41-05:00
---

# Description

[watermelon-full-preview] Diagnosed by a consumer team from /proc/locks after their disk-snapshot producer exited 75 three runs in a row and validate launches reported "configuration lock is busy". The mutation lock stack (`_mutation_locks`, py/wrkslots/cli.py) acquires in a fixed order — legacy project-root flock, then `<control>/config.lock` (`_locked_config`, #1808-D), then `ACTIVE.global.lock`, then `ACTIVE.<machine>.json` — under one shared deadline. So a `heartbeat --wait-lock 600` holds the config flocks while it queues for an ACTIVE.global WRITE, which readers (doctor, status phased reads holding shared ACTIVE locks) block; one of those status calls was itself waiting on the writer's registry intent lock (`ACTIVE.global.intent.lock`, the writer-fairness turnstile). Only the wait timeouts and the reader's 10-second intent deferral break the cycle. #1975 Phase 1 shortens the transfer-owner hold but does not change this order; the inversion is pre-existing (#1276/#1808 lock stack).

# Design

Starting point (2026-11-20): a consumer team's lane proposed a patch, relayed and sha-checked by the lead coordinator — teach `_run_prevalidated_mutation` a `release_config_after_check` pass-through and use it at both heartbeat lock sites (prevalidated rounds and the `_RegistryBusy` fallback), with a real-flock regression draft (heartbeat queued behind a shared-lock reader holds the writer turnstile but NOT the project-root flock; fails on 055d548a, passes patched, lock tests 31 → 32). Fold it in as the base of this change, credited in the commit message as "proposed by a consumer team's lane" (public history counts too — never the team's name). Adapt before applying: scrub host-specific incident detail from code comments (keep the mechanism, not the place/date), evaluate adopt and transfer-owner for the same treatment (the proposal covers heartbeat only; Phase 1 made transfer-owner a prevalidated mutation on the same stack), and re-based onto the landed Phase 1/6 tree.

Rule: never hold the config lock while waiting for ACTIVE.global. One global lock order, defined once and enforced:
(1) Inventory every holder-order in both editions (mutation locks, phased/shared readers, the intent turnstile, coordinator-adopted locks, release_config_after_check users) and write the single order down where `_mutation_locks` lives.
(2) For prevalidated mutations (heartbeat, adopt, transfer-owner), validate the configuration lock-free as today, then take ACTIVE.global before re-checking config under a short config hold — reusing or extending the existing `release_config_after_check` shape — so no path waits on ACTIVE.global with the config flocks held. Legacy-binary mutual exclusion (the reason the root flock leads) must be preserved or explicitly re-proved safe. Mixed-version constraint (lead coordinator, 2026-11-20): a pinned older wrkslots (096a8d74) shares the same state roots, so the chosen order must be deadlock-free in the cross product old×new, not only new×new — any reordering that only works when every party is new is not acceptable; if a clean reorder cannot satisfy that, the fix must instead break the cycle without inverting any pair the old binary can hold (for example by releasing and retrying rather than waiting while holding).
(3) A debug assertion (env-gated, off in production) records the lock-acquisition stack per process and fails on any out-of-order acquisition; the test suite runs with it on.
(4) Coordinate with the consumer team's proposed patch (arriving via the lead coordinator) — one change, not two.

# Acceptance Criteria

- A regression test with real flocks in a scratch state root reproduces the inversion (writer holding config while a reader holds shared ACTIVE and waits on intent) on the pre-fix code and shows all three parties completing after the fix, bounded by condition-based waits, never by the 600s timeout expiring.
- Mixed-version safety: a consumer runs a pinned older wrkslots (096a8d74) on the SAME state roots, so the new order must stay deadlock-free when one party is the old binary (old order) and the other is the new one. The regression test includes that pairing — a detached 096a8d74 worktree as the old party against the current binary, in both roles (old writer vs new reader, new writer vs old reader) — and every party completes or refuses by its own bounded wait, never by deadlock.
- No switched code path waits for `ACTIVE.global.lock` while holding either config flock (pinned by a lock-scope ledger test in the style of the heartbeat/transfer ones, both editions). Switched (release after the configuration check, both editions where the verb exists in both): heartbeat, adopt, transfer-owner, release, hold, unhold, write-handoff, read-handoff and supersede-handoff, plus the already-releasing prepared-removal and deferred-finish paths. Kept on the old mode, each with its reason recorded at the call site and in the fix commit: create, register and import-existing (records stamped with configuration TTL/layout/identity/caps), finish (provisioned-allowlist accounting), capture-drafts (Eden mount/ledger configuration), adopt-checkout (default remote/landed ref), init, quarantine and tail-event recovery (control-layout destinations), relocate-repository, clean-caches (cache globs), share-submodules, and the remove/retire/validate-batch and recover-* families (layout, salvage, remotes, hooks and validation policy judged from the configuration).
- The debug lock-order assertion is on in the test suite and off by default in production; a deliberately out-of-order test acquisition trips it.
- Existing lock-scope, owner-transfer, heartbeat, ownerless/adopt, and audit lock-free suites stay green; outputs and refusal strings unchanged apart from wait behaviour.
- No persisted fields change.

# Notes

Related: #1975 restore-boot-recovery (Phase 1 shortened the transfer-owner hold on the same stack; this issue is its own commit after Phase 1/6 land), #1276/#1808 (prevalidated mutations and the dual config lock), #1194 (config flock scope), #1987 wrkslots-rust-transfer-parity (Rust lock-scope gaps at cutover — the Rust side of this fix must not regress those). Starts only after Phase 1 lands. Number pre-assigned by the lead coordinator.

## Resolution

[watermelon-full-preview] Landed on main as a8cc8d82ef4b26d7c8660f5592d3f6e054bd8e98 (gated stack tip 23ee4242; PARTIAL gate row 1795733662788938941-5224ca83 under the 965-extension waiver, liaison 987). The mutation-lock order is now stated once at `_mutation_locks`: project-root flock, then `config.lock`, then `ACTIVE.global.intent`, then `ACTIVE.global`, then `ACTIVE.<machine>`. Configuration-independent prevalidated paths no longer hold the config flocks while queueing for the registry: heartbeat, adopt, transfer-owner, release, hold/unhold, write-handoff, read-handoff and supersede-handoff release after the configuration check, as the prepared-removal and deferred-finish paths already did; busy fallbacks re-check only `config_sha256` rather than full registry identity. Paths kept on the old mode carry their reason at the call site, and Rust create now takes the configuration locks before the registry locks. `WRKSLOTS_LOCK_ORDER_CHECK` records acquisitions and is enabled in the wrkslots test steps, with the enable decision cached and the release step budgeted for the measured cost. Real-flock and mixed old/new coverage lives in `py/wrkslots/tests/test_lock_order.py` and the lock-scope suites. No persisted fields changed.
