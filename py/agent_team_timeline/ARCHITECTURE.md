# Agent Team Timeline Architecture

This document is the cost and compatibility contract for `agent-team-timeline`. It must be updated
whenever a model prompt, model-visible input, generated output field, cache identity, or summary
selection rule changes.

## Durable project and export layout

The durable Hermit archive is `~/agent_logs_archive/summary/hermit`. One archive may contain
multiple teams under `teams/<team-slug>/`; source snapshots remain ignored, while normalized data,
expensive summary artifacts, model-usage receipts, and deterministic presentation files can be
version controlled. The intended teams for the first combined archive are `codex-coord-030`,
`claude-coord-176`, and `orc-coord-014`.

The durable archive and a website export are distinct concepts:

- The archive accumulates normalized team data and every validated expensive artifact.
- A slice export is a deterministic view over selected teams and a half-open UTC interval. It may
  contain only 2026-08-06 22:00 through 2026-08-07 07:00 EDT even though the archive retains more.
- Rebuilding an export never invokes a model. Missing summaries degrade to normalized transcript
  and statistics rather than causing unrelated cached summaries to be discarded.

The current implementation can store several teams in one archive, but `build` still renders one
team at a time into the shared top-level site. Combined-team build selection and independent slice
exports are therefore required before the Hermit archive is considered complete.

## Cost boundary

Fetching, SQLite snapshotting, normalization, artifact extraction, terminology candidate scanning,
phase construction, statistics, Markdown/JSON generation, and site rendering are deterministic
compute. They may be rerun freely.

Only six registered computations invoke a summary model. Their authoritative registry is
[`summary_registry.py`](summary_registry.py). Prompt version, output schema version, known version
history, inputs, outputs, and supported granularities live there. Prompt builders import those
versions; a prompt constant must not be invented elsewhere.

| Registered summarizer | Current version | Unit | Prompt generator |
|---|---:|---|---|
| `phase-work-summary` | 1 | one agent phase | `summarize.py:build_summary_prompt` (`phase` branch) |
| `agent-lifetime` | 2 | one agent lifetime | `naming.py:build_agent_name_prompt` |
| `project-overview` | 2 | one project/team knowledge epoch | `summarize.py:build_summary_prompt` (`project-overview`) |
| `glossary-definition` | 2 | one deterministic glossary term | `summarize.py:build_summary_prompt` (`glossary-definition`) |
| `technical-rollup` | 2 | day, week, month, or quarter | `summarize.py:build_summary_prompt` (`technical-rollup`) |
| `plain-language-rollup` | 2 | day, week, month, or quarter | `summarize.py:build_summary_prompt` (`plain-language-rollup`) |

Hourly rollups are a required next granularity, but are not listed as implemented until their
period construction, selection, persistence, and tests land.

### Phase work summary

Staged by `pipeline.py:_phase_jobs` after `phases.py:build_phases`.

Inputs:

- A fixed UTC phase (30 minutes by default) of one agent's messages and one-line tool aggregates,
  capped at 30,000 characters by retaining the front and back.
- Up to 16,000 characters immediately before the phase from that agent and every recorded
  ancestor, including the coordinator. This is a character budget, not currently a word budget.
- Deterministically detected glossary terms available before the phase ends.
- User-prompt, response, inter-agent-message, and tool-call counts.

Outputs are an at-most-80-character phrase, hover paragraph, and timestamped substantive work
bullets. Current projections are `summary_data/phases/<phase-id>.json`; immutable cache records are
`summary_data/cache/<input-hash>.json`.

Important correction to earlier design recollections: phase jobs currently receive ancestor raw
transcript context and the glossary. They do **not** receive arrays of prior daily and weekly
summaries. Prior calendar summaries are inputs to calendar rollups only.

### Agent lifetime name and summary

Staged by `pipeline.py:_agent_name_jobs` only after phase summaries exist.

Inputs are the official path, coordinator nickname, role, depth, parent path, the first selected
phase's ancestor context, and all available phase-summary phrase/paragraph/bullets for the agent.
Outputs are a hindsight short name, naming rationale, and one-to-three-sentence lifetime summary.
The independent cache is `summary_data/name_cache/<input-hash>.json`; projections are
`summary_data/agents/<thread-id>.json`.

### Project overview

Staged by `pipeline.py:_project_overview_job`.

The input is at most 48,000 characters from early root user/assistant conversation before a frozen
knowledge cutoff. Source event IDs, bounds, and a context digest are retained. The output is either
an evidence-supported newcomer overview or an explicit `Insufficient evidence` result. It may not
emit links. The projection is `summary_data/project_overview.json`; its first valid evidence epoch
is intentionally frozen against ordinary append-only growth.

### Glossary definition

Term candidates are found deterministically from root user prompts; that scan does not spend
tokens. One model job per term receives its exact spelling, the frozen project overview, and up to
six bounded source occurrences. It returns an evidence support status and a short definition, with
no invented links or acronym expansions. Definitions and evidence are projected together in
`summary_data/glossary.json`; model results use the common summary cache.

### Technical calendar rollup

Staged by `pipeline.py:_rollup_jobs_for_level` in chronological order for daily, weekly, monthly,
and quarterly levels.

Inputs are fully contained lower-level summaries, uncovered phase summaries at calendar
boundaries, up to ten already-completed earlier summaries of the same level, the chronological
glossary, and aggregate statistics. Thus a weekly job consumes daily summaries plus up to ten prior
weekly summaries; it does not directly receive an arbitrary independent array of ten prior days and
ten prior weeks. A daily job consumes phases plus up to ten prior daily summaries.

Outputs use the common phrase/paragraph/work-bullet schema and are projected under
`summary_data/rollups/<kind>/<key>.json`.

### Plain-language calendar rollup

This is a separate paid job, not formatting applied to the technical result. It receives analogous
lower-level and up-to-ten prior same-level plain-language results, plus the project overview and
supported chronological glossary definitions. It must identify the project for a newcomer and
describe content before opaque work-management identifiers. It shares each rollup projection with
the technical result but has an independent input hash and usage receipt.

## Common cache identity and receipts

Both runners content-address a job over the exact structured model payload plus registered
summarizer ID/version, output schema version, backend, model, prompt version, reasoning effort, and
non-default service tier. The default tier is omitted from the hash for compatibility with caches
predating explicit tier support. Batch size and worker count do not affect artifact identity.

Validated batches are published immediately in the shared version-1 envelope defined by
`summary_artifacts.py`. Every artifact records its deterministic artifact ID, logical key, team and
time interval, complete summarizer contract, model selection, context coverage, dependency keys,
generation time, and generating usage receipt. Every attempted backend batch has an immutable
usage receipt under the relevant cache's `_usage/` tree. The envelope version describes storage
shape and is distinct from the registered summarizer and output-schema versions.

The resolvers also try the former hash scheme. Valid summary-cache v1/v2 and naming-cache v3 files
remain hits without token spend; they receive in-memory provenance marked `legacy_storage: true`
and `unknown-legacy` context coverage. They are never rewritten merely to migrate storage.
Pre-lifetime naming-cache v2 remains incompatible because it lacks the required lifetime summary.

Projection records now carry the common provenance when it is known, while readers continue to
accept pre-envelope projections. A logical-key artifact catalog and newest-compatible selection
remain to be implemented before sparse mixed-version archives are fully inspectable.

## Standard staged-computation contract

Every registered summarizer will use the following common lifecycle:

1. **Stage input.** Materialize a deterministic job with logical key, team, half-open interval,
   registered summarizer ID/version, exact model-visible payload, dependency artifact IDs, and
   context-availability metadata.
2. **Hash.** Hash the canonical prompt contract, exact payload, backend/model/effort/tier, and
   output schema. Derived diagnostic metadata that is not model-visible is recorded but does not
   masquerade as a prompt input.
3. **Resolve.** Select an exact cache hit or submit a bounded batch. Validate every response against
   that registered output schema before publication.
4. **Publish immutable artifact.** Store the result, full version identity, dependency identities,
   context coverage, chronological frontier status, generation time, and usage-receipt ID. Never
   overwrite an artifact at a different content hash.
5. **Project.** Update a cheap logical-key index pointing to available artifacts. A projection may
   mix versions across time.
6. **Render.** Select the newest compatible artifact available for each logical key. Missing newer
   fields are absent/unknown in the UI, not fabricated and not a build failure.

Steps 1 through 4 use the shared runner contract for all six summarizers. Projection provenance is
also common. Step 5's logical-key catalog and Step 6's mixed-version selection remain the next
refactor; no production summary tokens should be spent before they land.

## Context completeness and frontier metadata

Each optional context channel records a requested count, provided count, and unit. Examples are
`ancestor_transcript` in characters, `prior_days` in summaries, `prior_weeks` in summaries,
`source_occurrences` in occurrences, and `project_overview` in artifacts.

Raw counts with unlike units are never added together. Each channel is converted to a 0–100%
ratio, and the simple `coverage_percent` is the equal-weight average of those channel ratios;
`missing_percent` is its complement. Component-level values remain authoritative so a later policy
can make a better decision than the simple score. A job with no optional context channels is 100%.

Calendar artifacts also record one of:

- `project-start`: no earlier project period was expected;
- `contiguous-extension`: the immediately preceding same-level artifact was available;
- `isolated-backfill`: earlier project time exists but the contiguous predecessor was absent;
- `unknown-legacy`: an older artifact lacks enough provenance to decide.

This metadata permits, but does not itself trigger, policies such as “regenerate only when context
coverage improves from 50% to at least 80%.” Automatic resummarization remains opt-in because it
spends tokens.

## Version compatibility policy

- A prompt or model-visible instruction change increments the registered summarizer version and
  prompt-version string, with a changelog entry.
- An additive output field increments the output schema version. Readers must accept older schemas
  and represent absent fields as unavailable.
- A semantic change to an existing field requires a summarizer-version bump even when JSON shape
  is unchanged.
- Old immutable artifacts and receipts are retained. A targeted run may upgrade only one hour or
  day; neighboring older versions remain valid inputs when their fields satisfy the consuming
  summarizer's declared minimum schema.
- A build never silently treats an incompatible old artifact as current. It reports version and
  context coverage, then degrades to the best compatible artifact or normalized source data.
- Source mutation/truncation and a cache hash mismatch fail closed. Ordinary append-only source
  growth creates new staged inputs only for affected windows/frontiers.

## Required time/backfill behavior

The durable archive must support independent summary selection from ingestion range. In particular:

- Generate or backfill one exact hour, day, or arbitrary half-open interval without buying every
  later summary.
- Record whether each calendar result extended a contiguous frontier or was isolated.
- Load sparse prior hour/day/week artifacts when available and record missing slots when absent.
- Sweep left-to-right later to improve context without making that sweep a prerequisite for a
  useful partial archive.
- Export one website slice without truncating or changing durable normalized data or cached
  artifacts.

The proving target is all three teams from `2026-08-07T02:00:00Z` through
`2026-08-07T11:00:00Z` (22:00 Aug. 6 through 07:00 Aug. 7 EDT). The first paid proving run is only
the first hour and uses `gpt-5.6-luna`; provider failure must be reported rather than silently
falling back to another model.
