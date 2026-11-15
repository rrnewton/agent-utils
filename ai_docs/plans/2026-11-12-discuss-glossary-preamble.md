# discuss-glossary-preamble: a two-layer warm-up for Discuss

Status: design for owner review (#1394 discuss-glossary-preamble). No build yet.
Feeds #1352 discuss-warmup-context (`catchup_warmup_files`, `assemble_warmup`,
`load_warmup`); measured against #1351 discuss-unknown-terms-metric and the
#1393 discuss-unknown-terms-log records.

## Problem

Discuss starts cold on two kinds of vocabulary. The logged unknown terms
(owner, Slack 013/023) mix company-wide systems words with project words:
systems examples included filesystem, packaging, incident and
source-control terms; project examples included lane, coordinator, tool and
agent names. One flat glossary cannot serve both: the first kind changes
rarely and is shared by every channel, the second changes weekly and is
specific to this project. The owner asked for both, common terms only, with
esoteric one-offs skipped.

## Design: two layers, one warm-up

Both layers are plain files named, in order, by the existing
`[summaries] catchup_warmup_files`. `assemble_warmup` already renders them in
order under one budget with per-file `## Reference:` headings, and
`load_warmup` already fingerprints path+mtime+len and reloads on change, so
no new serving mechanism is needed. Order is the design:

1. **Layer 1 — general systems primer (first, stable).** A short list of
   common infrastructure and process terms that recur across channels:
   incident review, the source-control virtual filesystem, devservers,
   packaging and deploy pipelines, the model-tool protocol, and similar.
   One line per term: term, expansion, one clause of what it means here.
   Rarely edited (monthly at most), so it forms the stable head of every
   Discuss prompt.
2. **Layer 2 — project glossary (second, weekly).** Lane and tool names,
   coordinator names, and agent names with their #1342 purpose lines,
   regenerated weekly (below). This tail changes; putting it after the
   primer keeps the longest possible prompt prefix byte-identical between
   runs.

### Where each layer lives

The mechanism and the formats live in agent-utils; the content does not.

- Agent-utils ships only an empty/illustrative example (the existing
  `vibe-talk/prompts/discuss-glossary.md` stays as the format example) and
  the generation script described below. No company, host, channel, or
  consumer names appear in this repository.
- The real Layer 1 primer is consumer data. It lives in the consumer's
  state or harness (for example beside the deployment's other operator
  files, next to the store), is edited by the operator, and is named by
  `catchup_warmup_files` first. It is never committed here.
- The real Layer 2 output likewise lives in the consumer's state
  directory, regenerated there, and is named second. Only its generator
  and format are specified here.

### Size split

The #1352 budget stays 6,000 characters (about 1,500 tokens):

| Layer | Budget | Shape |
|---|---|---|
| 1 systems primer | ~2,000 chars | 25–35 terms, one line each |
| 2 project glossary | ~4,000 chars | terms first, then agents |

Within Layer 2, definitions come before the agent list, and the agent list
is cut first when the budget runs out: a name without its purpose line
teaches the model nothing. `assemble_warmup` cuts at the whole budget in
file order, so the split is enforced by keeping each file within its own
share, not by a code change.

## (a) Deterministic agent list

Generated from the agentctl registry (#1342 records carry a one-line
purpose per agent):

- **Source.** The registry's recorded agents: name plus purpose line,
  exactly as recorded; no inference, no model call, so the output is
  byte-deterministic for a given registry state.
- **Which agents.** Live agents first (registry-alive), then agents seen
  in the last 7 days, each marked only by being in that order; stopped
  agents older than 7 days are omitted. The window is a parameter, not
  a timestamp baked into the file, so regeneration is reproducible from
  a registry snapshot.
- **Privacy.** Only name and purpose are emitted. No host names, paths,
  session ids, or working directories leave the registry, and the
  generated file itself lives in the consumer's state, never in this
  repository.
- **Freshness.** Regenerated with Layer 2 weekly, and also whenever the
  registry's purpose lines change materially (a fingerprint over
  name+purpose pairs, checked by the generator, makes a no-change run a
  no-op write).

## (b) Weekly "top terms" glossary

- **Candidates, deterministically.** Sweep, in order: the #1393 log's
  unknown-term frequencies (`unknown-terms-report`), project docs,
  recent commit subjects, and issue titles. Count case-insensitively;
  a term is a candidate at 3 or more sightings, or 2 unknown-log
  sightings. This threshold is the owner's "common only" rule made
  mechanical: one-off esoterica never qualify.
- **Definitions.** Model-written, one line each, from the term's own
  sighting contexts (the sentences it appeared in), then human-reviewed
  before the file is served: the generator writes a draft, the weekly
  job stops there, and an operator's edit is the publish step. No
  definition is served unreviewed, because a wrong gloss is worse than
  an unknown term.
- **Systems terms route to Layer 1.** A candidate that is not specific
  to this project is proposed for the primer instead, which is how the
  primer stays current without growing weekly churn in the stable
  prefix.
- **Size.** Layer 2 terms are capped by the budget above; candidates
  beyond it wait, ranked by count, so the file cannot grow past its
  share.
- **Where it lives.** Consumer state, beside the primer; regenerated
  weekly by a scheduled job the consumer runs (the generator script and
  the report command ship here, the schedule and the output do not).

## Turn the warm-up on?

Yes — now, with the starter glossary, answering #1352's open question.
#1352's n=3 measurement could not settle value, and #1393 now gives the
live instrument (`unknown_term_count` beside `warmup_fingerprint`, plus
the JSONL log) to settle it on real runs instead. The cost is bounded
(about 1,500 tokens, under a second), the mechanism is already deployed
off, and off-by-default has produced no further evidence in a week.
Recommendation: operator sets `catchup_warmup_files` to the primer and
the starter project glossary, turns on `catchup_unknown_terms_log`, and
reviews the weekly report before committing to the generated Layer 2.

## (c) Prefix caching

Low priority at a 1–2.5k-token preamble, and not cheaply checkable today:

- Discuss calls are stateless: `Summarizer::complete_prompt` sends system
  plus prompt per call (`summarize/meta_gateway.rs`), with no session to
  restore and no cache-control field on the wire shape this server
  builds. Reusing one gateway session would be a new backend feature,
  not a configuration change.
- No cache evidence is available to check: the Discuss stamp carries
  `input_tokens: None`, and the gateway responses this server parses do
  not surface a cache-hit field, so two timed calls could show only
  noisy latency, not a hit. Saying more would be guessing.
- The free version is already in force: a fixed preamble order (primer,
  then project, then labels legend, then window) is the longest stable
  prefix a provider-side automatic cache could use, and it costs
  nothing. If token accounting later exposes cached-token counts,
  re-measure then; do not build session reuse for this preamble size.

## Owner 094: id lookup and an agentic definer, under the broader glossary effort

Status: design only, awaiting the owner's go (Owner 094, 2026-11-15). No
build is authorised by this section. The unknown-terms log that motivates
it holds 9 runs and 42 distinct terms; the latest run's unknowns were
mostly tracking ids (`S5b`, `S5c-2`, `GO1`, `T-REAL`, `L1`, `CAS`). The
owner's direction is that anything sophisticated here belongs to the
broader glossary, labelling, topic-classification and summarization
effort, not to Discuss alone. Discuss is one consumer of that shared
glossary; this section specifies two candidate sources for it and where
they sit. The pipeline placement is designed in the [voice-overview
plan §4–§5](2026-11-07-voice-overview/README.md) and the snapshot and
convergence design in [shared vocabulary](2026-11-08-shared-vocabulary/README.md);
both link back here.

### (a) Deterministic candidate selection: look the id up where it was introduced

The weekly sweep in (b) above counts sightings in the log, docs, commits
and issues. For tracking ids that rarely helps: an id is coined once,
in one message or one transcript line, and then used bare. The
deterministic step therefore adds a targeted lookup before any model is
asked to define anything:

- **Corpora, in order.** (1) The full message history for the channels
  in scope, searched further back than the Discuss warm-up window (the
  warm-up window is a prompt budget, not a search bound). (2) Agent
  transcripts: the session logs the hosts already keep for their coding
  agents (interactive and headless sessions alike), searched as text.
  Both corpora are already retained for other reasons; no new capture
  is proposed.
- **Hit selection.** For each unresolved term, collect exact,
  case-insensitive sightings and prefer the *introducing* one: the
  earliest message or line that defines the id in place — patterns such
  as `S5b = …`, `S5b: …`, “S5b (… )”, or a sentence that expands or
  names what the id tracks — over later bare uses. Keep the introducing
  sentence plus a small window of surrounding context as the evidence
  record. If no introducing sighting exists, keep the earliest sighting
  and mark the candidate `context-only`.
- **Output.** A candidate is `{term, evidence refs, introducing text if
  any, sighting count}` — evidence, not a definition. It enters the same
  review queue as the (b) sweep's candidates; nothing is served from a
  raw hit without the definition and acceptance steps below.
- **Privacy and scope limits.** Search stays inside the deployment's
  own retained history and transcripts; only the introducing sentence
  and its immediate context leave the corpus, never whole transcripts.
  No host, channel or consumer names are written into this repository:
  corpus locations are consumer configuration, named beside the other
  consumer state. Terms whose only evidence is text the owner never
  saw stay proposal-only until accepted (the shared-vocabulary
  bootstrap rule), so an id coined between agents cannot silently
  become owner-facing vocabulary.
- **Cost.** Exact-match text search over retained logs; no model call.
  The expensive failure mode is an unbounded history scan per term, so
  the lookup is batched per glossary run (one pass per corpus, all
  unresolved terms at once) and bounded by a configurable lookback.
  Expected cost is negligible next to one definition call.

### (b) Agentic definer: propose cited definitions for the residue

Whatever (a) cannot resolve — no introducing sighting, or only
`context-only` evidence — goes to an agent acting as a glossary
collaborator, not as an oracle:

- **Task.** Given the term, its sighting contexts from (a), and access
  to the same corpora (message history, transcripts, docs, commits,
  issues), propose a one-line definition in the Layer 1/Layer 2 format,
  or answer `insufficient-evidence`.
- **Citations are mandatory.** Every proposed definition carries the
  source refs it rests on (message, transcript line, doc, commit or
  issue). A proposal without a citable source is not written to the
  queue. The definer must never invent an expansion or a meaning: an
  uncited guess is worse than an unknown term, for the same reason (b)
  above gives for unreviewed definitions.
- **Acceptance gate.** Proposals land as `proposed` in the shared
  glossary store and are published only on acceptance by the owner or
  a designated curator. Acceptance, rejection and edits are recorded
  with the citations, so a wrong gloss can be traced to its evidence.
  Nothing the definer writes is served to Discuss, injected into a
  prompt header, or published in a snapshot before that gate.
- **Where the output goes.** An accepted definition is an ordinary
  glossary entry: it flows to the Discuss warm-up (Layer 1 or 2 by the
  routing rule in (b) above), to the published snapshot, and to every
  other consumer on the same terms as a human-written entry. The
  definer is a writer into the gate, never a publisher.

### Placement: shared glossary first, Discuss second

Both steps feed the one shared glossary store (voice-overview §4.2):
(a) is a deterministic candidate source alongside the unknown-term
frequencies, docs, commits and issues already listed in (b); (b) is the
definition step for candidates that have evidence but no gloss, behind
the same acceptance gate and epoch publication as every other writer.
Discuss then consumes the result exactly as designed above — primer
first, project glossary second, inside the existing warm-up budget —
with no Discuss-private definition path. Building either step for
Discuss alone, with its own store or its own acceptance rule, is
explicitly out of scope; if the broader effort reorders its stages,
these two move with it.

## Acceptance for the eventual build

- Generator emits Layer 2 deterministically from a registry snapshot
  plus the sweep sources; same inputs, byte-identical output.
- No company or host specifics in agent-utils; shipped examples are
  illustrative only.
- Warm-up byte budget unchanged; primer first; `make validate` green.
