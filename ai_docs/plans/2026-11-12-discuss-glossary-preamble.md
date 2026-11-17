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

## Owner 131 (2026-11-17): unique plan ids, plan registry, agent-name tokens

*Brainstorm from the owner in thread 131 (2026-11-17 ET); no go yet. This
section extends the two-layer glossary design above with three things the
owner asked for after reading an overnight recap full of opaque phase
labels: plan phases that carry a unique, linkable name; a Discuss agent
that knows who the subagents are; and agent names written as a distinct
token. The liaison's proposed order is kept: agent-name tokens first,
then Discuss context, then registered plans, link rendering, and the
recap rule. Sizes are rough and for ordering only.*

### The problem, in the owner's words

Agents keep falling back to planning labels that only make sense inside
the plan that invented them ("wave 1, round 3", "step 2A"). The reader
cannot tell what the label names, cannot click through to its
definition, and cannot even tell that it is a plan reference at all. The
fix the owner sketched: when an agent invents phases at planning time,
make it name them uniquely from the start, register the plan, and let
the infrastructure turn each such name into a link to its definition.
The same treatment should make agent names recognisable at a glance.

### 1. Unique plan ids: `name$phase`

**Syntax.** `prefix$ID`, where `prefix` is the plan's short name
(lowercase letters, digits, hyphens; registered, see below) and `ID` is
the phase label the planner already wants to use (`1A`, `2A`, `W1`).
Example shape: `storage-rework$1A` — the storage-rework plan, phase 1A. The full
token is the only spelling agents use in prose; the bare phase label
alone is never enough.

**Why `$`.**

- It almost never appears in ordinary prose, so finding it needs no
  word-context guessing (contrast the bare-number guard for thread
  references, which needs local resolution to be safe).
- It is inert in both Markdown and Slack's message format: not a link,
  emphasis, heading, mention, or short-reference character, so it
  survives every converter unchanged and can be matched with one simple
  pattern.
- It does not collide with the existing token families: `#` numbers
  (issues), `:thread:`-style short references, `<@…>` mentions, and
  the proposed `@@name` below.
- Alternatives considered and set aside: `/` reads as a file path and
  breaks when a plan name itself contains one; `!` reads as emphasis
  or a command; a second `:` would blur into the short-reference
  family. If the owner dislikes `$`, any single rare punctuation
  character keeps the same properties; the design only needs one.

Matching rule everywhere: the pattern above, outside code fences and
inline code (the same exclusion the bridge already applies when it
scans reply text).

### 2. The plan registry

One small file per deployment, in the consumer's state directory beside
the other Discuss warm-up files — never in this repository, for the
same reason the real glossary content stays out (see "Where each layer
lives" above). Suggested name: `plan-registry.json`. Shape:

```json
{
  "schema": "plan-registry/v1",
  "plans": {
    "storage-rework": {
      "title": "Storage rework",
      "definition": "ai_docs/plans/storage-rework.md",
      "phases": {
        "1A": { "gloss": "Move the index to the new layout",
                "anchor": "phase-1a" }
      }
    }
  }
}
```

- `definition` plus `anchor` is where the phase is defined: usually a
  section of a plan document; the owner noted it will generally be a
  tracker issue but may be any linkable Markdown. The registry stores
  the address once; prose only ever carries the short token.
- `gloss` is one line, written by the planner at planning time. It is
  what the reader sees without clicking.
- **Registration.** The agent that writes a plan registers it in the
  same step (a tiny command or a hand-edited entry; a checker confirms
  the definition file and anchor exist). No registry entry, no unique
  id — the phase keeps its bare label and the lint below objects.
- **Lifetime.** Entries are append-mostly: a finished plan's entry
  stays, so old recaps keep resolving. Retiring a plan marks it done;
  it does not delete the mapping.

### 3. How the infrastructure binds an id to its definition

Every consumer loads the same registry file the same way the Discuss
warm-up already loads its files: read-only, cached, re-read when the
file changes, and a missing file simply means "render plain". No
network call on any delivery or display path.

- **Chat bridge (Slack).** On the outbound reply path the bridge already
  rewrites recognised tokens; a registered `prefix$ID` becomes a link
  to the definition with the token as its label. An unregistered one is
  left plain and reported (see the lint).
- **vibe-talk.** The web page renders a registered token as a link
  whose hover text is the one-line gloss; clicking opens the definition
  (a document in the page, or the external address for a tracker
  issue).
- **Discuss.** The registry's titles and glosses join the warm-up (see
  section 5), so the side agent can expand a token in its answer
  instead of repeating it, and the unknown-terms count gains a way to
  fall: a token the registry defines is no longer unexplained jargon.

### 4. The poke: unregistered or bare phase labels

A checker on the same paths that already scan agent text (the bridge's
outbound scan; a standalone lint for documents and recaps):

- A `prefix$ID` token with no registry entry: warn the author —
  "register the plan or rename the phase" — through the existing
  warning channels (the reply command's output, the service log, the
  status warnings list). Never block the send.
- A bare phase-shaped label (`2A`, `W1-r3`) with no prefix: report
  only, never rewrite — like parenthesised bare numbers, it can be too
  many other things to rewrite on sight. Reports are what drive the
  habit change; the planning instructions carry the rule itself:
  *name phases uniquely at planning time and register the plan before
  first use.*

### 5. Discuss context: the generated who-is-who

This is Layer 2 of the design above, made concrete, plus the registry:

1. **Who-is-who, generated.** From the agent registry: each live
   agent's name and its recorded one-line purpose, then agents seen in
   the last week. Deterministic, no model call, regenerated weekly and
   whenever the purpose lines change. Only name and purpose leave the
   registry.
2. **Fixed context.** The general systems primer (Layer 1) and the
   project glossary (Layer 2 terms), exactly as designed above.
3. **Plan registry summary.** Each registered plan's prefix, title,
   and phase glosses, cut last under the existing warm-up budget.

All three ride the warm-up mechanism that already exists (named files,
one budget, fingerprint reload); there is no new serving path. If the
generated context makes plan and agent references comprehensible to
the side agent, the same glosses are what the human-facing rendering
shows — one source, two readers.

### 6. Agent-name tokens: `@@name`

**Syntax.** In agent prose, a subagent's name is written `@@name` (the
agent's registered name after the double sign). Plain `@name` stays
the chat mention it already is; doubling the sign marks "this is one
of our agents" without a lookup, and `@@` is as rare in prose and as
inert in Markdown and Slack as `$` is above.

**Rendering.**

- **vibe-talk and the bridge:** a coloured pill (a rounded chip in the
  project's agent colour) showing the name, with the agent's purpose
  line as hover text; in vibe-talk, clicking the pill can open the
  same who-is-who detail the chat `!who` answer gives. On Slack, where
  pills do not exist, the token renders as the name in a distinct style
  with the purpose appended on first use in the message.
- **Discuss:** the who-is-who warm-up (section 5) is keyed by the same
  names, so the side agent resolves `@@name` instead of guessing.
- **Fallbacks.** An unknown `@@name` renders plain and is reported by
  the same checker as an unregistered plan id. The token is written by
  agents and coordinators in their own prose; nothing rewrites a bare
  name into a token automatically, because names are also ordinary
  words too often to rewrite safely.

### 7. Order and rough sizes (liaison's order)

| Step | What | Rough size |
|---|---|---|
| 1 | Agent-name tokens: convention, bridge + vibe-talk pill rendering, checker report | A few days; rendering only, no new store |
| 2 | Discuss who-is-who: the weekly generator (registry purposes + glossary + plan titles) feeding the existing warm-up | A few days; the warm-up path already exists |
| 3 | Registered plans: registry format, registration checker, planning-instruction wording | A few days; one file format, one small checker |
| 4 | Link rendering for `prefix$ID` in the bridge, vibe-talk, and Discuss answers, with the one-line gloss | A few days per surface; reuses the existing token-rewriting passes |
| 5 | Recap rule and lint: first use of a phase id is linked or glossed; bare labels reported | Small once steps 3–4 exist; mostly wording plus the report |

Steps 1 and 2 help on their own and need nothing from the later steps.
Step 3 is the commitment device: after it, steps 4 and 5 are
mechanical.

### 8. What already exists

- **Discuss warm-up mechanism: built.** Named reference files are
  prepended to the Discuss request under a character budget, with
  fingerprint reload and per-run logging. It ships off until an
  operator names the files.
- **Unknown-terms measurement: built.** Every Discuss answer's
  "terms I didn't understand" are parsed, counted, and (behind a
  switch) logged server-side with a frequency report — the instrument
  the glossary work is measured against.
- **Starter glossary: built.** A short shipped example file in the
  format the warm-up consumes.
- **Agent purpose lines: built.** Each agent's record carries a
  one-line purpose, shown in status listings and the `!who` detail —
  the exact input the who-is-who generator needs.
- **This two-layer design: written, awaiting the owner's go.**
- **Thread/message short references (`:thread:` style): built.** The
  bridge writes `:thread:NNN` / `:email:NNN` / `:issue:NNN` references
  as links (#658), and vibe-talk renders a thread or message reference
  as one short linked label (🧵/✉️ plus the number; #1607, #1713,
  deployed 11-17). The `prefix$ID` and `@@name` tokens are deliberately
  shaped to sit beside that family, not inside it.
- **Clickable mention links: built (links); popup pending.** Mentions
  render as links to the person's profile or team page (#1722, deployed
  11-17); the @-targeting popup (#1721) awaits the owner's go. The
  `@@name` pill is a separate, smaller surface and does not wait for it.
- **Plan registry, `prefix$ID`, `@@name`: not started.** Nothing in the
  tree registers plans or recognises either token.

### 9. Open questions for the owner

1. The separator: `$` as proposed, or another rare character? And is
   `@@` the right agent-name sign?
2. Must every plan register, or only plans whose phases get cited in
   owner-facing recaps? (Proposal: every plan that names phases at
   all.)
3. Warning strength: report-only to start (as proposed), or should an
   unregistered id in an owner recap eventually hold the recap?
4. Gloss placement: on first use per message (as proposed for Slack),
   every use, or hover-only where the surface allows hover?
5. Who owns the registry file day to day — the coordinator of the team
   that writes the plans, with the checker as backstop?
6. Should clicking an agent pill open the who-is-who detail, the
   agent's live session, or nothing (hover only) in the first cut?
