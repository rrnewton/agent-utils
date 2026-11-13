---
title: 'wrkslots-ttl-teardown-ignores-live-owner-and-hold: ttl teardown deletes live-owned and held slots'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
- ttl
created_at: 2026-11-13T15:00:00+00:00
updated_at: 2026-11-13T15:00:00+00:00
---

# Description

[watermelon-full-preview] `py/wrkslots/cli.py` `_cmd_ttl_teardown` (#1293, also on github/main) judges only the TTL stage from `heartbeat_at + heartbeat_ttl_seconds + teardown rung` (`_ttl_slot_stage`), then rescues and deletes the slot. `_execute_ttl_slot_rescue` likewise. Neither refuses a slot whose recorded owner process is LIVE nor honours a wrkslots HOLD. A slot whose agent is working but has not run `wrkslots heartbeat` for a day reaches the teardown rung and would be deleted. A consumer has a live-owner slot whose `teardown_at` is 2026-11-14 15:10Z.

# Design

Both editions checked; `rs/wrkslotsd` has no ttl-teardown / ttl-rescue path, so the fix is Python-only (parity is vacuous). 1) `ttl-teardown` and ttl rescue (`_execute_ttl_slot_rescue` / `_execute_ttl_slot_teardown`) REFUSE, by name, a slot whose owner is live (same judgment `remove`/`finish` use: pidfd-first `_process_state` #1300 Phase 2a plus `_owner_liveness_states` registered/validation authority) and a slot under an active HOLD (same semantics as `remove`: `_load_hold`, event log authoritative). Re-check both under the mutation locks right before the commit. 2) `ttl-report` marks such slots `blocked: owner live` / `blocked: held` instead of presenting `teardown_at` as scheduled; stage may still say teardown/expired. No wall-clock waits; tests inject `now` and liveness.

# Acceptance Criteria

- Live owner past the teardown rung -> refused by name, nothing removed, no rescue side effects beyond a read-only report.
- Held slot past the rung -> refused by name.
- Dead owner, no hold, past the rung -> torn down exactly as today.
- Owner becomes live / hold added between lock-free gather (rescue) and commit -> refused at the under-lock re-check.
- `ttl-report` (human/json/health) shows `blocked: owner live` / `blocked: held` for such slots.
- Scratch project roots only; never run unlanded wrkslots against a shared registry or a real slot.

# Notes

Related: #1293 TTL ladder, #1300 Phase 2a pidfd-first evidence, `remove` HOLD/liveness semantics. Consumer `teardown_at` 2026-11-14 15:10Z is the P0 deadline; no consumer names in this file.

## Resolution

[watermelon-full-preview] TTL teardown/rescue now refuse by name when the recorded owner is live or the slot is held: _ttl_blocked_reason/_assert_ttl_not_blocked (pidfd-first _process_state #1300 2a + _owner_liveness_states, _load_hold as in remove) run first regardless of TTL stage in _execute_ttl_slot_rescue/_execute_ttl_slot_teardown/_cmd_ttl_teardown, and again under the mutation locks before commit. ttl-report keeps stage but shows next_action expired (owner live) past expiry (blocked field = blocked: owner live) / blocked: held, with no ladder step for live owners. Heartbeat help documents the non-owner ancestry refusal (heartbeat from the holding sub-agent). Tests in py/wrkslots/tests/test_ttl_guard.py cover live/held/dead, any-stage, report labels and both re-check flips with injected now/liveness; scratch roots only.
