# shared vocabulary: where the glossary lives, and how agents converge on it (design)

Design only, 2026-11-08. Companion to the voice-overview design
(`2026-11-07-voice-overview`, especially §4 Glossary and §8.5). Nothing here is
implemented. Owner asks: replies 226–228.

## Phone summary (one screen)

A glossary kept only inside vibe-talk would teach the *reader* names the
*coding agents* never see — owner reply 226's point. So the vocabulary needs a
published form agents consume, not just a store the page reads.

**Recommendation.** Keep the glossary *store* in vibe-talk for now (it owns the
labelling pipeline, the acceptance gate and the used-with-owner facts), and
**publish a versioned glossary snapshot** — one JSON file, epoch-stamped — that
everything else reads: mu-prnames' curated-names file is its first export
target, and the agentctl chat bridge injects its top-K as a compact
"current vocabulary" block in the prompt header. Deterministic rendering
(links named at render/post time) does most convergence work for free; the
header block teaches; a propose-only lookup CLI lets agents ask. Extract a
standalone component (the logbook direction) only when a third writer appears.

Summarization itself should be shared **below the prompt** — one model-backend
transport, cache identity, receipts and limiter (mu-spark's config-swappable
gateway proves this layer in shipped code) — while prompts, units and grouping
stay per-family: chat threads (human↔agent groupings) and transcript
time-chunks are different domains that share machinery, not summaries.
wrkviz should **not** go live: zero-token incremental ingest at poll cadence,
summarization stays batch/on-demand.

**Bootstrap:** the human-visible chat layer is authoritative for names (it is
where you learn them, and where stability promises live); PR/code output
anchors identities deterministically; raw transcripts only *propose*
candidates. Minimal first step: snapshot v1 + curated-file export + bridge
header behind a flag, and measure whether agents actually start using the
names. Owner picks: Q14–Q17 in the voice-overview summary.

## 1. Where the shared vocabulary lives

Three homes were compared (reply 226's (a)/(b) plus the snapshot hybrid):

| Axis | (a) vibe-talk store + API | (b) standalone component | Snapshot file (recommended first) |
|---|---|---|---|
| Writers today | pipeline + owner already there | none — all migrate | vibe-talk writes; all read |
| Agent access | HTTP read (bridge fetches) | crate/CLI per consumer | file read / embedded copy |
| Freshness | live | live | epoch-stamped (weekly + on correction) |
| Failure mode | server down ⇒ no vocabulary | adoption cost up front | stale-but-present, says its epoch |
| Precedent | name cache lives in vibe-talk now | logbook proposal (unbuilt) | pr-names curated file + P2 shared cache file |

**Take the snapshot first, decide the store later.** The store question is
really "who may write, and who adjudicates same-thing" (§4 of the overview
design) — today exactly one writer pipeline exists, inside vibe-talk, so
moving the store now buys an extraction with no second writer to serve. What
agents and other tools need is not write access to that store; it is a
*stable, bounded, versioned read* of it. A snapshot gives that without
coupling every consumer to vibe-talk's availability or schema churn:

- `glossary-snapshot.json` (schema-versioned): `{epoch, generated_at,
  entries: [{key (kind-prefixed, §8.5), slug, display, sentence, gloss,
  aliases[], kind, status, used_with_user}]}` — accepted/owner-pinned entries
  only; proposals never publish. Bounded by the §4.4 caps at the source.
- Published at each epoch roll and immediately on owner correction (rename,
  merge, pin) — corrections are exactly the changes agents must not keep
  missing for a week. Written tmp-then-rename next to the other server state;
  a copy may also be served read-only over HTTP for hosts that don't share a
  filesystem (same bytes, `ETag = epoch`).
- Consumers state the epoch they used wherever it matters (bridge header line,
  render stamp), so "which vocabulary did that agent see?" is answerable —
  the telephone-game rule applied to vocabulary itself.

Migration path, stated so the choice stays reversible: when a second *writer*
appears (wrkviz batch proposals are read-mostly and don't count; a writer means
accept/retire authority), extract the store into the standalone component —
which by then has a proven file format, two-plus consumers, and the pr-names
lane's own "extract at the third consumer" rule as precedent. Option (a)'s API
survives either way as the read path; option (b) is the destination, not the
first step (Q14).

## 2. How agents see it: feedback channels, ranked

Convergence must not depend on agents remembering (reply 224's principle).
Four channels, strongest first; ship 1+2, add 3 on demand, reject 4 as primary:

1. **Deterministic rewriting at render/post time (already the pr-names
   design).** Links render `REPO#N slug` whatever the agent wrote; P2 carries
   the same enrichment to bridge/mirror posts. Zero agent cooperation needed;
   this alone fixes what the *owner* reads. It does not teach agents — hence 2.
2. **Compact vocabulary block in the chat-bridge prompt header (the teacher).**
   On session start (and only then — not per message), the bridge prepends, in
   its existing sender-header family: epoch line + top-K entries ranked by
   retention score then channel relevance, inside the §4.4 ~1,500-token budget
   (in practice K≈25–40 entries), formatted exactly as the labelling prompts'
   known-terms block (`- slug [aliases]: sentence`) so one format serves
   models on both sides. Refresh cadence: new sessions get the current epoch;
   a running session is not re-injected mid-conversation (vocabulary changing
   under a live agent mid-task is worse than one-epoch staleness). Behind a
   bridge flag first (Q15), with the epoch recorded so divergence is auditable.
3. **Lookup/suggest CLI (pull, propose-only).** `glossary lookup <words>`
   (exact/alias/fuzzy over the snapshot — offline, file-local) and
   `glossary suggest <slug> --evidence …` filing a *proposal* into the §4.2
   gate queue. Agents may ask and propose; they may never write, rename or
   retire. A skill teaches the two commands; the deterministic channels above
   mean the system still works if no agent ever calls them.
4. **Periodic vocabulary-update messages — not as a channel.** Posting vocab
   diffs into chat threads pollutes exactly the threads the overview exists to
   summarize, and reaches agents only if they happen to read that channel.
   Epoch changes belong in the coordinator's existing digest/heartbeat line
   ("vocabulary epoch 12: +2 terms, 1 renamed"), never as chat traffic.

Two-way flow, summarised: pipeline + owner → store → snapshot → {page chips &
voice glosses, pr-names curated export, bridge header, lookup CLI}; fetched PR
titles, transcript coinage candidates and agent suggestions flow back only as
*proposals/evidence* into the gate. Nothing downstream writes upstream (§8.4).

## 3. One summarization component? Share below the prompt (reply 227i)

Prior art is unusually aligned here. mu-spark's gateway work (`a1da8df4` +
`148867ec`, in-flight at writing) proves the transport layer is already
model-swappable by pure configuration over vibe-talk's shipped `Summarizer`
trait — three backends, one limiter, backend+model+effort folded into cache
identity, no per-model branch. The logbook proposal (synthesis appendix 2)
generalizes exactly that layer — `ModelBackend` transport, content-addressed
artifacts, receipts, freshness, capability-checked reads — under a `Store<M>`
parameterized by chat vs transcript metadata. wrkviz today shares none of it:
CLI-subprocess backends at 20–24 s/job, its own prompt registry and
payload-addressed cache (same *discipline*, separate code).

**Recommendation: extract the below-prompt layer when logbook work next
moves — transport, cache identity, receipts, limiter, fencing — and keep
everything at/above the prompt per-family.** What can be one implementation:
HTTP model clients, the identity tuple (summarizer id+version, schema, backend
identity, exact payload), token receipts (wrkviz has them; chat must gain
them), spend ceilings, prefix-digest ingest cursors. What cannot: prompt text
and summarizer registries (thread naming ≠ phase summary ≠ rollup), budget
*policy* (interactive per-view vs nightly `--plan`-gated batch), and the
sub-calendar groupings below.

**wrkviz live? No (Q17).** Its batch shape is load-bearing: build/export must
spend zero tokens, summarize is budget-gated with resumable partial state, and
the CLI-per-job path cannot meet interactive latencies. "Live" worth having is
deterministic and already specified by the synthesis: tail-seek cursors making
ingest cost O(appended), polls at minute cadence, summarization on demand or
nightly. mu-archive's `excavate` design lands the same way (daily zero-token
excavation; summarization a downstream consumer, explicitly out of scope).
Morphing wrkviz into a daemon would spend the one thing its archive guarantees
— reproducibility from cached artifacts — to save latency nobody asked a
*timeline* for. The voice overview's freshness needs are met by vibe-talk's
own pipeline (§5), not by a live wrkviz.

## 4. Two domains: threads are not time-chunks (reply 227ii)

| | Chat (vibe-talk) | Transcripts (wrkviz / archive) |
|---|---|---|
| Natural grouping | thread (human-started), channel | session lineage; 30-min phases; calendar periods |
| Text unit | message (human-visible, edited/deletable) | speech record (prompt/response/inter-agent), append-only |
| Assignment today | thread slug by model, kept | project by deterministic launch-evidence rules |
| Summary ladder | message → thread → (hour, proposed) | phase → hour/day/week rollups |
| Freshness need | seconds–minutes (a person waiting) | hours–days (a reader browsing) |

mu-archive's bucketing is the cautionary mirror of Orc's clustering (§8.3):
assignment there is *rules over launch evidence* (override → agent identity →
git remote → cwd containment → content-as-borderline-only → other), one primary
project per lineage plus cross-references — content deliberately never assigns,
because its job is custody ("whose work, may we delete the raw?"), where a
wrong bucket loses data. Chat workstreams answer a different question ("what is
this about, for a listener?") where content is the only signal that exists —
threads carry no cwd/remote. So: share the *shape* (exactly-one-primary +
cross-references/mentions, stable IDs, config-hash-stamped assignments,
borderline queue instead of forced labels — §5.2 already mirrors all four),
never the assignment *mechanism*. A session's archive project and a thread's
workstream may legitimately disagree; cross-referencing them (same slug where
the owner accepts it in both registries) is a glossary alias, not a merge.

## 5. Where terms bootstrap: three sources compared (replies 227iii/228)

| Source | What it mints well | Failure mode as authority | Cost/latency |
|---|---|---|---|
| Raw agent transcripts | richest coinage (tools, subsystems, errors agents actually say) | names the owner never saw; volume (speech is 0.1–2.5% of 224 GiB); privacy of unseen text | batch mining; cheapest per candidate |
| Agent↔human messages (chat) | names in actual shared use; thread slugs already live here | sparse for deep/technical terms agents only use with each other | already ingested; interactive pipeline exists |
| PRs & code output | exact identities (owner/repo#N, title slug prefixes, paths) | titles are sentences, not slugs; no conversational meaning | deterministic fetch, no model needed for identity |

**Recommendation: chat authoritative, code anchoring, transcripts proposing.**
A term becomes *vocabulary* — stable, glossed, protected once used with you —
only at the human-visible layer, because every promise in §4.5 (stability,
first-use glossing, "same term tomorrow") is a promise about what you hear and
read. PR/code output contributes identity and exact-rule candidates for free
(`pr` entries, `#123 slug` references, title prefixes — §8.5). Transcripts are
the best *candidate miner* (wrkviz batches proposing terms bottom-up, §4.2)
and must stay proposal-only: a term coined from text you never saw arrives
unrecognised, however accurate — the precise failure §4.5's glossing budget
exists to ration. Combinations in evidence order for coinage adjudication:
owner/curated > code-anchored exact > chat-attested (≥2 threads) >
transcript-only (needs chat corroboration or owner acceptance to publish).
This also keeps the privacy boundary simple: nothing enters the published
snapshot whose evidence is solely owner-invisible text without an explicit
acceptance step (Q16).

## 6. Minimal first step, and what to measure

1. Snapshot v1 writer in vibe-talk (accepted entries only, epoch file beside
   the store) + export of `pr` slugs to mu-prnames' curated file on epoch roll.
2. Bridge header block behind a flag (top-K, epoch line), snapshot read at
   session start; no mid-session refresh.
3. `glossary lookup` over the snapshot file (read-only; `suggest` later).
4. Measure before building more: fraction of agent messages already using
   canonical slug forms (deterministic rewrite makes the *rendered* number
   100%; the interesting number is what agents *write*), proposal acceptance
   rate by source (chat vs transcript vs code), and header-block token cost
   per session. If agents converge without steps 2–3, stop at step 1 — the
   deterministic layer was sufficient, which is reply 224's bet.

Open beyond Q14–Q17: snapshot hosting for multi-host fleets (file per host vs
HTTP read); whether wrkviz batch proposals may cite archive project slugs as
workstream aliases automatically (recommendation: proposal evidence only, §4);
and the logbook extraction trigger (recommendation: first third writer, or the
next wrkviz transport change, whichever comes first).
