# Rust-primary parity inventory: agentctl, wrkviz, shared schemas

Lane: mu-rsctl (slot rsctl, branch mu-rsctl/rust-primary-agentctl-wrkviz).
Base inventoried: 5bb572f1 (github/main, 2026-11-07). Issues: #238
agentctl-rust-only, #545 rust-primary-agentctl-chat-readers, #706
rust-primary-wrkviz, #723 rust-primary-schema-collapse.

Headline: the brief's starting picture is stale in a good direction.
Inside `agentctl` exactly one command is still Python-only (`migrate`);
`watch`, `restore`, `watch-agents`, tab order, `chat history/context/
launch`, MCP, `reset` and `repair` are already Rust. `wrkviz` has every
top-level command ported in Rust but is still Python-default. Only 3 of
22 inventoried formats are schema-collapsed. The long pole is the
`herdr-subagents` foreign headless-worker port, which also carries
`agentctl migrate`.

## #238 + #545: agentctl

Router: `agentctl` primary is Rust (`validation/editions.json`,
`validation/editions.tsv`); the Python mirror is kept at parity.
Rust refuses Python-only commands by name with exit 2
(`rs/agentctl/src/cli.rs:2617`: `PYTHON_ONLY_COMMANDS = ["migrate"]`,
`PYTHON_ONLY_CHAT_COMMANDS = []`). Python refuses the Rust-only set
(`restore`, `install-units`, `watch`, `watch-agents`, `hold`,
`coordinator`, `manage`, `set`, `tree`, `progress`, `services`) with
exit 2 (`py/agentctl/cli.py` `_RUST_ONLY`).

| Surface | Rust | Router default | Differential coverage | Python LOC left |
|---|---|---|---|---|
| Core lifecycle (start/adopt/list/status/send/drain/read/wait/stop/rename/move/revive/restart/goal/tree/manage/set/coordinator/hold/progress/services/skill/profiles/quickstart, ~60 commands) | full | Rust | `cross/agentctl_differential.py` (3,282 LOC, 27 groups, shards 0-2/3) | mirror only; `py/agentctl` is 42,399 LOC total, mostly this mirror |
| `watch`, `watch-agents`, `restore`, `install-units`, tab order | full (`rs/agentctl/src/restore.rs`, `agent_watch.rs` 10,750 LOC, `tab_order.rs`) | Rust; Python refuses | no named cross-differential group; Rust tests only | refusal only |
| `chat` (all 30 subcommands incl. `history`, `context`, `launch`) | full (`rs/agentctl/src/cli.rs:1694`) | Rust | `py/tests/test_agentctl_chat_history_rust.py` (2), `test_agentctl_chat_context_rust.py` (2), `test_agentctl_chat_launch_rust.py` (14) | mirror `py/agentctl/chat.py` 5,702 LOC at parity; Python polling-launcher formats refused by design |
| MCP (`agentctl mcp`) | full shell (`rs/agentctl/src/mcp.rs` 420 LOC) | Rust | `py/tests/test_agentctl_mcp_rust.py` (1) | 147 LOC mirror |
| `reset`, `repair` | full for Rust headless agents; partial for foreign turn-runner workers | Rust | `test_agentctl_reset_rust.py` (1), `test_agentctl_repair_rust.py` (2) | shared with herdr-subagents gap |
| `migrate` | none | Rust refuses, exit 2; no differential group | none | `py/agentctl/sessions.py:355` -> `worker_rpc.py` -> `foreign/agent_migrate.py` + `foreign/lib.py:3802` |
| `herdr-subagents` standalone (up/send/read/status/down/inbox/reset/migrate/backend/turn/keeper/mcp) | none; Rust refuses non-Claude headless workers (`rs/agentctl/src/cli.rs:4010`) | Python (`herdr-subagents python/absent`) | none; Python tests `test_herdr_subagents.py` 2,169 LOC + `foreign/tests/` | `py/agentctl/foreign/` 8,265 LOC ex-tests (12,414 with tests) - largest remaining gap |
| `herdr-chat` standalone | none as entrypoint; function superseded by Rust `agentctl chat`, but `GoogleChatTransport` (`py/agentctl/chat.py:1505`) has no Rust counterpart | Python (`herdr-chat python/absent`); explicit Rust request refuses exit 127 | none | ~8k LOC chat stack shared with the chat mirror |
| `herdr-agent` | full, primary Rust | Rust | `cross.agentctl.herdr-agent-differential` | mirror only |

Stale record to fix in the first landing: the `herdr-chat` note in
`validation/editions.json` still says Rust chat lacks
launch/history/context; it has had them since the #545 slices above.

## #706: wrkviz

Router: `wrkviz` primary is still Python; `wrkvizd` (Rust) is primary
with no Python mirror. `WRKVIZ_ENGINE=rust|rs` opt-in wins over generic
overrides (`common/bin/engine-resolver:122` -> `rs/bin/wrkvizd`);
unset keeps Python; a generic Rust request without the opt-in refuses
exit 127; invalid values exit 2. Totals: Python 60,889 LOC, Rust
76,500 LOC, 28 differential files / 186 tests, all passing at the last
recorded gates.

| Command group | Rust | Differential (cases) |
|---|---|---|
| `inspect`, `serve`, `quickstart`, `--userguide`/`--help`/`--version` | full | 10 / 4 / 2 / 5 |
| `query list/show/search/prompts/messages/stats` (incl. `message:`/`tool:` show refs) | full except schema-2 archives | 8 / 6 / 4 / 3 / 5 |
| `extract-transcripts` (schema 3 + 1) | full | 9 |
| `ingest`, `ingest-claude`, `ingest-claude-history`, `ingest-muse`, `ingest-orc`, `ingest-project` | full, via `rs/agent-harness` decoders | 9 / 10 / 13 / 7 / 12 / 7 |
| `summarize`, `build`, `export` (single + multi-team), `gc` | full | 14 / 10 / 5 / 6 |
| `migrate-build-state`, `migrate-snapshots`, `github-metadata`, `refresh` family | full | 3 / 3 / 2 / 6 |
| `audit-glossary`, `audit-losslessness`, `responsiveness`, `discover-agents` | full | 5 / 5 / 6 / 7 |

Remaining wrkviz work, ordered:

1. Schema-2 timeline reader for `query`/`inspect` (refuses at
   `rs/wrkviz/src/query.rs:493`): L, ~1.5-3k Rust LOC + differentials.
   This was the only true Python-only command surface left until item 2.
2. Added 2026-11-08, Python-only: `summarize --agent-scope coordinator`
   (coordinator-only summary tree under `summary_scopes/coordinator/`)
   and the `summarize-project` subcommand (project weekly from team
   weeklies). The Rust edition prints the same help and usage but refuses
   both by name with exit 2. A recurring coordinator-only weekly run
   breaks at the A3 default flip unless these are ported first, or that
   run pins `WRKVIZ_ENGINE=python`: M, summary tree plumbing plus
   differentials. Tracked as #1077 rust-wrkviz-agent-scope-port.
   Related, both editions: #1076 site-weekly-item-stamps (the site's
   week-level rollup pages still print invented times of day; pages for
   a narrowed scope already label items by day, ISO week or month).
3. `--userguide` divergence (invalid-args subcommand after
   `--userguide`): S, ~50-150 LOC.
4. Accepted gaps (non-finite/oversized-int JSON, lone surrogates,
   Python `re` features, codec wording): M, piecemeal, none falls back.
5. Edition flip to Rust-primary with a `WRKVIZ_ENGINE=rust` soak first:
   S in code, gated on coordinator/owner approval; static site assets
   carry over in the same step.
6. Python archive (60,889 LOC) only after the flip soaks.

## #723: shared schemas

Source of truth: `validation/editions.json` `formats[]` (22 formats).
`ai_docs/plans/2026-11-01-schema-collapse/PLAN.md` is stale (says
no schemars use; slices 1-3 have landed). Landed pattern per format:
one Rust type -> schemars schema in `validation/schemas/` ->
regenerate-and-diff checker (+ self-test) in the validation DAG ->
Python test-only validation (hand-rolled validator; `jsonschema` is
not vendored).

Collapsed (3): agent-queue items/tombstones/quarantine
(`rs/agent-queue/src/schema.rs`, 3 schemas); wrkslots state/handoffs
(`rs/wrkslotsd/src/formats.rs`, 4 schemas, partial: config/hold/
journals remain); wrkviz schema-3 bootstrap (`rs/wrkviz/src/formats.rs`,
1 schema, partial: snapshot pointer + streamed records remain).

Duplicated, in this lane's scope first: registry record `agent.json`,
coordinator designation, coordinator host index (Rust-vs-Rust, two
writers), landing queue, launch spec, launch/goal journals,
move/rename/revive journals (plus uninventoried
`agentctl-fresh-revive/v1` and `agentctl-cloud-input/v1`), profiles,
restore config/report, watch state, headless runner records, chat
bridge state (~12 schema consts), chat plugin/Slack payloads. There
is no shared formats crate yet; creating that home gates the agentctl
slices. Calibrated sizes: S 100-200 LOC, M 200-450, L 450-900, XL
split required, plus ~132 LOC per checker.

## Landing sequence (coordinator decisions 2026-11-07, clock ET)

Two parallel tracks, not one serial queue. Google Chat byte-parity is
explicitly not required: the owner left Google Chat on 2026-10-26 and
only the Slack instance is live, so `GoogleChatTransport` is retired
(its data and config untouched) and `herdr-chat` is shimmed or retired.

Track A (wrkviz, #706):

| Step | Content | ETA (ET) |
|---|---|---|
| A1 | Schema-2 timeline reader + differentials | 2026-11-10 17:00 |
| A2 | `--userguide` divergence fix | 2026-11-11 17:00 |
| A3 | Default flip: `WRKVIZ_ENGINE=rust` default, `python` opt-out, static assets; Python kept one release | 2026-11-13 17:00 |
| A4 | Archive `py/wrkviz` after ~1 week clean | 2026-11-20 17:00 |

Track B (agentctl, #238/#545, #723 woven in):

| Step | Content | ETA (ET) |
|---|---|---|
| B0 | Fix stale `herdr-chat` editions note | 2026-11-07 17:00 |
| B1 | Differential groups for watch/restore/MCP | 2026-11-11 17:00 |
| B2 | `herdr-chat` shim/retire, Google transport retired | 2026-11-13 17:00 |
| B3 | `herdr-subagents` port split per harness adapter (shared core/ownership, then Codex, Claude, Muse/agy adapters and MCP/keeper in parallel) + `agentctl migrate`; registry/launch schema collapse woven in | 2026-11-27 17:00 |
| B4 | Archive `py/agentctl`: re-point differential oracle to frozen goldens, resolver/editions updates; remaining chat-bridge/agent-teams/dagrun schema collapse | 2026-12-05 17:00 |

Finish line: agentctl and wrkviz Rust-only, Python editions archived,
schemas generated from single Rust types. Each step lands separately:
Rust implementation, a durable differential test in `py/tests/` kept
with the project, router flip only at proven parity, and the full
gate (`python3 scripts/validate.py --all --keep-going`, bare
`scripts/check_client_names.py`, `cargo fmt --check`) on the exact
push tree.
