---
title: 'wrkslots-rebind-agent: hand a slot whose agent is proven dead to a differently named agent'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-07T16:20:00.000000000+00:00
updated_at: 2026-11-07T16:20:00.000000000+00:00
---

# Description

A slot records one agent name for life. `wrkslots adopt` and `wrkslots transfer-owner` both take `--agent <NAME>`, which must be the name already recorded on the slot, and change only the owner pid. Giving an idle slot to a new agent with a different name therefore takes `remove` plus `create`.

On 2026-11-07 that cost two extra slots on one host. An owner-ordered retire freeze forbade `remove` and reclaim, and new lanes were started inside idle slots whose records still named their old, dead agents. That defeats liveness and attribution: the record says the old agent owns the slot while another agent works in it, and a later cleanup could treat the live lane's work as the old owner's. The fix was to raise the slot cap by two and create fresh slots, leaving the idle ones held.

# Design

- `wrkslots transfer-owner SLOT --agent OLD --to-agent NEW --owner-pid PID --expected-generation N`: allowed only when the recorded owner is provably dead under the same liveness proof transfer-owner already requires. It also needs a clean tree, or the slot's existing preservation rules applied first.
- Record the rename in the slot journal: the old and new agent, who ran it, and the liveness evidence. Bump the generation so stale callers are refused.
- Keep refusing when the old owner is alive or its liveness is unverifiable.
- Both editions: the Python wrkslots and wrkslotsd must agree.

# Acceptance Criteria

- A test: a slot whose agent is dead is re-bound to a new agent name; status shows the new agent and the new pid; the journal records the rename.
- A test: re-binding is refused while the recorded owner is alive, and while its liveness is unverifiable.
- The Python and Rust editions agree (differential or paired tests).

# Notes

Filed 2026-11-07 by the devbig030 coordinator at the lead's request. The retire freeze is what made it bite: without the freeze, remove plus create works, at the cost of a new slot image. Related: #993 (pane attribution in liveness).

## Resolution

[watermelon-full-preview] `transfer-owner --agent OLD --to-agent NEW` now rebinds a slot to a differently named successor in both editions. `--agent` remains the recorded name and refuses on mismatch; a rename requires a recorded owner that is provably dead (no recorded owner, a live owner, or an unverifiable one all refuse), the usual successor ancestry/slot-box proof, and a clean working tree in every present checkout (branch unchanged, empty status outside configured caches), so uncommitted work is never silently reattributed. The rename bumps the generation, records `previous_agent`/`previous_generation` with the existing owner evidence on the usual `owner-transferred` event, and composes with `--reopen` (a released generation renames the same way). Covered by the differential rebind cases in `py/wrkslots/tests/test_transfer_owner_write_rust.py` (match, live-owner refusal, wrong old agent, dirty-tree refusal) comparing ACTIVE and events across the editions.
