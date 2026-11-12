---
title: 'agent-purpose-line: a one-line description of what an agent is for'
status: closed
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-12T11:45:00+00:00
updated_at: 2026-11-12T23:10:00+00:00
closed_at: 2026-11-12T23:10:00+00:00
---

# Description

[watermelon-full-preview] Owner request (reply 404, via coordinator coordb, lane mu-agentmeta, decided after #1341 agent-metadata-audit): do we have any one-line description of what an agent is doing or is for, based on its original brief — and does an agent self-describe on any surface? Audit answer: no. `agent.json` has a `goal` field, null on 27 of 27 measured records; slots record a `--purpose` at creation that nothing shows; briefs are delivered as prompts and never distilled.

Record a one-line purpose per agent at `agentctl start` and show it where people look: `agentctl status`/`list`, the `!who` detail answer, the headless console banner, and (as a separate careful step) the Herdr tab title.

# Design

## Which field: a distinct display-only purpose, NOT `goal`

Coordinator-approved after the audit. `goal` is a delivered harness objective: `agentctl goal` stores it and immediately prompts the agent with it (`/goal …`, or a "Your ongoing goal:" prompt), tracks `goal_delivery`, and binds native Codex goals. A display label must never instruct, queue, or wake the agent, so purpose gets its own key:

- New extension key `purpose` (constant beside `standing`/`viewer_tab_id` in `subagents/control.rs`), read/written through the same nested-extensions/flat-extra paths, so there is no record schema bump and Python load/save preserves it.
- `AgentRecord::purpose()` / `set_purpose(Option<&str>)` mirror the viewer-tab accessors. A record without a purpose carries no key.

## Sources at start, in order (never blocks a launch)

1. Explicit: `--purpose TEXT` on `agentctl start`, generalized. Today `--purpose` is agentcloud+`--provision`-only (a provisioned node's roster label, `cli.rs`); keep that passthrough for cloud provisioning and record the same value locally for every harness. It is launch metadata, not a launch setting: it no longer conflicts with `--profile`, and an explicit value overrides a profile's cloud purpose.
2. Slot: with `--slot NAME` (not a project box), the slot's recorded wrkslots purpose, taken from the status row the post-launch owner bind already reads (`wrkslots status --slot NAME --format json` inside `bind_slot_owner`), so it costs no extra wrkslots call; a refused or purposeless bind falls through to the brief and never fails the start. It outranks the brief line recorded at creation, but never an explicit `--purpose`.
3. Brief: the first meaningful line of `--brief`/`--file` text — first non-empty line with heading/bullet markers stripped and whitespace collapsed, sender-header lines skipped, truncated to 200 characters.
4. Nothing: no key. A short model summary may come later; it is out of scope here and must never block start.

Resolution is one pure function over the three sources so tests pin the order and the failure fall-through without spawning anything.

## Command

`agentctl purpose NAME` prints `{name, purpose, previous: purpose}`; `agentctl purpose NAME "TEXT"` sets it; `agentctl purpose NAME --clear` removes it. Modelled on `agentctl set` (`ManagedAgents::set_purpose` in `control.rs`, same lock/load/save shape; unchanged text writes nothing). Validation follows the goal precedent: non-empty after trim, single line, at most 200 characters. Printing the agent, the new purpose and the previous one as JSON. Python edition: Rust-only command on the `set` precedent — help row plus refusal via `_RUST_ONLY` in `py/agentctl/cli.py`.

## Surfaces

- status/list: `record_row` emits `purpose` when set (standing/viewer-tab precedent), in both editions (`py/agentctl/subagents.py` mirror, so the status differential stays byte-identical).
- `!who` detail: after #1339 bang-answer-markdown lands, `BangAgent` gains `purpose` from the record key in `snapshot()` (top-level or `extensions`), rendered only in the detail view as a `- purpose:` bullet after managed-by; the tree stays terse. Seamed with #1339's lane (mu-threadbind): no snapshot/render clash. Until #1339 lands, ship the same line in the current detail render so the feature is not blocked.
- Console banner: headless console's opening line (`console.rs`) appends the purpose when set (`agentctl console NAME (PURPOSE): …`), factored into a tested helper.
- Herdr tab title: NOT in this build. Tab label equals the agent name today and is an identity proof in tab verification, rename, hygiene and duplicate checks (at least: `subagents.rs` label comparisons in the 3400s, 8800s rename, 9100s hygiene, 9300s anchor paths; 32 tab-label call sites). A `NAME — purpose` label is recorded here as an explicit follow-up decision with those citations so a later change updates every proof at once; silently splitting the label here would break agent identity checks for unrelated lanes.

## Tests (kept with the project, smallest focused set)

- Accessor round-trip: set/read/clear on flat and nested-shaped records, key absent when unset.
- Source resolution: explicit beats slot beats brief beats none; brief-line extraction (blank/heading/bullet/header lines, truncation); a failing slot lookup falls through.
- Manager: `purpose` command JSON shape (`{name, purpose, previous}`), unchanged set writes nothing, validation refusals.
- Status row emits purpose only when set (Rust + Python mirror).
- `!who` detail shows the purpose line; tree does not.
- Console banner includes purpose when set, unchanged when unset.
- CLI: `--purpose` accepted for a non-cloud start and recorded end-to-end in a fixture start; the pre-existing fixture behaviours stay green.

# Acceptance Criteria

- `agentctl start NAME --purpose "…"` records the one line in the agent's record for every harness (agentcloud keeps its provisioning passthrough); slotted starts without `--purpose` record the slot's wrkslots purpose; brief-only starts record the brief's first meaningful line; a start with none of the three records no key and never fails for a purpose reason.
- `agentctl purpose NAME` / `NAME "TEXT"` / `NAME --clear` reads, sets and clears with `{name, purpose, previous}` JSON; invalid text (empty, multi-line, over-length) is refused with nothing written; updating purpose never queues, prompts, or wakes the agent (`goal` untouched).
- `agentctl status NAME` and `list` show `purpose` when set, byte-identically in both editions; `!who NAME` shows the purpose line in detail; the headless console banner shows it; the `!who` tree and Herdr tab label are unchanged.
- Focused tests above fail before the change and pass after; the full gate lands it on main.

# Notes

Decided with the coordinator from #1341 agent-metadata-audit: purpose is display-only and distinct from `goal`. Reuse seam from #1340 agent-model-unknown noted for future usage work (not needed here). Tab-title follow-up carries the identity-proof citations in Design above. Herdr tab label, `!who` tree format, and `goal` semantics are explicitly unchanged by this issue.
