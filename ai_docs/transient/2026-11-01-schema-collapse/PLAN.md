# Generated-schema collapse: plan (rust-primary campaign)

Standing rule: every on-disk/wire format has exactly ONE Rust type as
source of truth, with a versioned generated schema checked in; Python
mirrors validate against the generated schema, never hand-maintain a
second definition.

## Inventory (current state, verified 2026-11-01)

No crate in rs/ uses schemars/JsonSchema today; no generated JSON
Schema is checked in. Formats and their defining types:

| Format | Rust source of truth | Python mirror today | Duplicate risk |
|---|---|---|---|
| agent queue items / registry records | rs/agent-queue/src/queue.rs, registry.rs | py/agentctl reads/writes JSON directly | Python hand-maintains field names |
| agent.json / coordinator.json | rs/agentctl registry_layout.rs, agent.rs | py/agentctl | two hand-kept shapes |
| landing-queue.json | rs/pr-landing-planner | py mirror if any | low |
| wrkslots state / handoffs | rs/wrkslotsd registry/event types | py/wrkslots/cli.py | highest line count, both editions write |
| wrkviz archive records (team.json, timeline/search shards) | rs/wrkviz types | py/wrkviz | both editions write archives |
| chat bridge state (chat.json) | rs/chat-subscription + agentctl chat_runtime | retired Python polling state refused | low (refusal pinned) |
| teams.json / declarations | rs/agent-teams | py tests fixtures | medium |
| validation JSON (editions.json, components.json) | scripts/check_editions.py + checkers | same files | Python is the checker; Rust resolver reads TSV |

## Proposed sequence (one gated slice each)

1. agent-queue first: smallest, already a single shared Rust crate.
   Add schemars derives, generate schemas/agent-queue/*.schema.json,
   check in; Python validates queue/registry JSON against it in the
   differential harness. Proves the generation + validation plumbing.
2. wrkslots state/handoffs: highest writer risk; do after (1) proves
   the pattern. Fixture-only writers, per writer rule.
3. wrkviz archive records: after (2); archives are versioned already
   (schema-3), generation must emit that version.
4. agent.json/coordinator.json, teams.json, landing-queue: as their
   tools' slices come up.
5. validation JSON stays Python-checked; Rust reads the generated TSV
   (already the pattern editions.json -> editions.tsv).

Each slice: generated schema checked in, a checker step proves the
checked-in schema matches the Rust type (regenerate-and-diff), Python
mirror validates against it, differentials unchanged in behaviour.

## Non-goals for the first slice
- No format version bumps; schemas describe current wire format.
- No default flips, no publishing, no parallel-experiment-runner.

## Status
Slice 1 (agent-queue) implemented as above; slices 2-5 pending
coordinator direction after slice 1 lands.
