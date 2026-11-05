# Throughput ordering for the merge queue: what SubmitQueue (EuroSys '19) does, and what we should build

Status: design note only. No code. Written from the paper itself:
Ananthanarayanan et al., "Keeping Master Green at Scale", EuroSys '19,
DOI 10.1145/3302424.3303970 (15 pp., CC BY; copy read:
`submitqueue-eurosys19.pdf`, sha256 `dac26206...595d`).
Section numbers below are the paper's. Queue file references are to
`rs/merge-queue` on agent-utils `main` at `17b31c28` unless said otherwise.

## 1. The paper in one page

SubmitQueue guarantees an always-green mainline while hundreds of changes per
hour land. Three engines do the work (paper §3, §6, Fig. 4):

- A **planner engine** asks, every epoch, which builds to run with the workers
  available, starts the most valuable ones, and *aborts running builds the
  newest list no longer contains* (§6). Aborting on a dropped likelihood is a
  first-class move, not an error.
- A **speculation engine** maintains a speculation tree/graph over pending
  changes, annotated with success probabilities, and returns builds ordered by
  expected value (§4).
- A **conflict analyzer** decides which pending changes are independent, which
  trims the speculation space and lets independent changes commit in parallel
  (§5).

The build controller then runs only the *minimal* set of build steps (steps a
prior build already ran are eliminated), balances them over workers by
historical step durations, and caches artifacts (§6). Our queue has analogues
of the first two ideas in weaker form, and none of the third at target level.

### 1.1 The speculation tree (§4, Fig. 5)

For pending changes `C1, C2, C3` in submission order, write `B1` for the build
of `HEAD+C1`, `B1.2` for `HEAD+C1+C2`, and so on. Speculating on every outcome
takes `2^n - 1` builds for `n` changes, and only `n` of them can ever be used
(§4.1). SubmitQueue instead annotates each tree edge with the probability the
path is needed, and schedules by **value of build** (§4.2.1):

```
V(B)         = benefit(B) * P_needed(B)        (benefit = 1 for all builds)
P_needed(B1) = 1
P_needed(B1.2)   = P_succ(C1)
P_needed(B2)     = 1 - P_succ(C1)
P_needed(B1.2.3) = P_succ(C1) * (P_succ(C2) - P_conf(C1,C2))
```

where `P_conf(C1,C2)` is the probability that `C2` passes alone but fails
combined with `C1` (§4.2.1, Eqs. 1-5). A build that becomes irrelevant when a
predecessor resolves is stopped (§4.1, §6). Because edge values only decrease
along a path, a **greedy best-first** walk orders the graph in `O(n)` space
even for hundreds of pending changes (§7.1).

Our rolling window is exactly one root-to-leaf path of this tree: each batch
is built on the previous batch's candidate, assuming every predecessor lands
(the paper's "Optimistic" baseline, §8, Zuul-style). When the head goes red we
cancel everything behind it and rebuild - which the paper predicts is the
expensive failure mode of optimistic speculation.

### 1.2 The success-probability model (§4.2, §7.2)

`predictSuccess(Ci)` and `predictConflict(Ci,Cj)` are supervised logistic
regression models trained on historical changes that passed through
SubmitQueue and their final outcomes (§7.2). About 100 hand-picked features,
in four families:

| Family | Features (paper's list) |
|---|---|
| Change | affected-target count; commits in the change; binaries added/removed; files changed; lines/hunks added or removed; status of initial tests/checks run before submitting |
| Revision | times changes were submitted to the revision; revert and test plan suggested in the revision |
| Developer | developer name; employment length; level |
| Speculation | counts of speculations that succeeded / failed so far for the change |

Training was a 70/30 split in scikit-learn; reported accuracy 97%, then
recursive feature elimination cut the set to the minimum (§7.2). Strongest
positive weights: number of succeeded speculations, revision revert and test
plans, number of initial tests that succeeded before submitting. Strongest
negative: number of failed speculations, number of times changes were
submitted to the revision (§7.2). The authors report performance close to an
oracle that foresees every outcome when the estimates are right (§4.2.1, §8).

Two things matter for us. Developer identity carries real predictive power
(their words: it "helped tremendously" for conflict chance too), and the
speculation family is dynamic - the model is re-scored as builds resolve, so
early predictions being wrong does not lock the schedule (§4.2.2, §7.2).

### 1.3 The conflict analyzer (§5)

Conflicts are decided on **build targets**, not files. Each target gets a hash
over its dependencies' hashes and its source files' contents (Algorithm 1).
A change's affected set is the targets whose hash changes, closed over
dependents. Two changes conflict if their affected sets intersect on a target
name - with a second, composition check (Eq. 6) for the case where applying
both produces a target hash seen in neither single-change graph (§5.2,
Fig. 8). The exact check needs about `n^2` build graphs, so the paper uses a
union graph of `HEAD`, `HEAD+Ci`, `HEAD+Cj` (`n` graphs) and propagates
"affected" marks topologically; a target marked by both means conflict (§5.2).
Only 7.9% (iOS) resp. 1.6% (backend) of changes alter the build graph at all,
which is what makes the cheap path sound (§5.2).

Why it matters: with 2 concurrent potentially-conflicting changes there is
already a 5% chance of a *real* conflict; with 16, 40% (§3, Fig. 1).
Independent changes collapse the tree into a graph where `B1` and `B2` run in
parallel and either can commit first (§4.2.2, §5, Figs. 6-7: one example drops
7 candidate builds to 5).

### 1.4 The numbers it reports (§8)

Evaluation replays iOS-monorepo changes at 100-500 changes/hour against four
baselines, normalized to an oracle with perfect prediction (§8.1-8.3):

| Result | Number |
|---|---|
| SubmitQueue P50 turnaround vs oracle | at most 2.98x slower; 1.2x at 500 workers for 500 changes/hour |
| SubmitQueue P95 turnaround vs oracle | up to 4x under high contention; falls with provisioning |
| SubmitQueue throughput slowdown vs oracle | up to 60% under high contention; ~20% at 500 workers; matches oracle at 100-200 changes/hour |
| Speculate-all turnaround | P50 up to 15x oracle, P99 up to 24x; more workers do not help (graph too deep) |
| Single-Queue (Bors-style serial) | worst: P50/P95/P99 80x/129x/132x at 500 changes/hour; 95% throughput slowdown |
| Optimistic (our current shape) | throughput flat as workers grow: bounded by the longest contiguous run of successes (< 100 in their workload) |
| Conflict analyzer effect | up to 60% P95 turnaround improvement for oracle; substantial for SubmitQueue and speculate-all; ~20% for optimistic |
| Before SubmitQueue | iOS mainline green only 52% of one measured week; "remained green at all times" after launch (§8.5) |

## 2. Mapping onto our queue

| Paper piece | Our analogue today | Gap |
|---|---|---|
| Speculation tree, value-ordered | Rolling stacked window (`run.rs` `pipeline`): one optimistic path, AIMD width (+1 per landing, suspect-red halving) | No per-build value; every slot assumes all predecessors land; red head cancels the whole suffix |
| Planner aborts stale builds | Red-behind-head cut (`cut_after`), refusal refill (landed `d425e407`), early-red slot release (landed `17b31c28`) | Aborts happen only on red/refusal, never because a better build appeared |
| `predictSuccess` / `predictConflict` | Decision records (`decisions/*.jsonl`) carry `predicted_pass` - **null in all 113 records** of 2026-11-04/05 | No model, no features beyond the record's `features` block, no training loop |
| Conflict analyzer on targets | Merge-tree probes (`plan.rs`): pairwise "does not combine" + file paths; probe cache; `ci/test-footprints.json` exists on the consumer side | Textual only: two changes that touch disjoint files but one binary look independent; same-file changes that compose cleanly look conflicting |
| Minimal build set / step reuse | Selection policy (`validator.select`) + remainder runs of failed tests only | Selection is per-batch, not per-target delta between stacked candidates |
| Greedy best-first over graph | Fixed plan order: priority, conflict degree, age (`plan.rs` rank) | Order is static for the run; no re-ranking as results arrive |

The cheapest faithful step is not the model. It is the **graph**: stop
treating the window as one path. The machinery to build candidates on
arbitrary parents already exists (`build(parent, prs)`); the window only ever
passes the stack top as parent.

## 3. Does expected-value build selection subsume the ABC+D diversity rule?

Short answer: **yes in substance, no on any timescale that helps this month.**
Item 3 as specified (one stacked chain plus at least one batch disjoint from
it, "ABC and D") is a single hard-coded point in the space the paper's
algorithm searches.

Why it is subsumed in substance:

- With `P_conf(Ci,Cj) = 0`, the paper's own math prefers validating `Ci` and
  `Cj` separately in parallel over validating them combined
  (`P_succ(B1.2) = P_succ(C1)*P_succ(C2)`, §4.2.2) - a disjoint slot is what
  value-ordering *derives*, not an exception to it.
- With conflict edges present, the speculation graph (Figs. 6-7) still keeps
  independent changes on parallel roots while stacked builds continue above
  them. "Keep one chain, add one disjoint batch" is the graph restricted to
  two roots and depth from only one of them.
- Value ordering also decides *which* change gets the disjoint slot (the one
  whose combined build has the lowest `P_needed`), and when to abandon it -
  both are guesses in the hard-coded rule.

Why it does not subsume it yet, concretely. The algorithm needs four inputs
the queue does not have:

1. **Per-change success probabilities.** Decision records have the outcome
   side (113 validations, 2026-11-04/05: 15 green, 24 red, 56 refused,
   18 cancelled) but `predicted_pass` was never populated, and the feature
   block records only lines/files/components/conflict-degree/priority -
   nothing about the author, prior attempts on the same head, or focused
   tests already run.
2. **Pairwise conflict probabilities.** The probe cache knows textual
   combinability for pairs it has probed. It records no *semantic* conflict
   outcome (pair validated together, failed, each passed alone), which is
   the training label `predictConflict` needs.
3. **A target-level conflict graph.** Our conflict signal is merge-tree
   textual. The paper's is affected-target sets. The consumer already
   generates `ci/test-footprints.json` (per-test file footprints), which is
   the raw material for an affected-set approximation: two changes conflict
   to the extent their footprints-over-changed-files intersect. Nobody feeds
   it to the queue today.
4. **Build cost per candidate.** Value in the paper is benefit x probability
   with benefit uniform. Our validations differ by an order of magnitude in
   wall time (batch wall median 215 s, mean 795 s in the same records), so a
   useful value function needs the recorded `cost.wall_s/cpu_s` history per
   selection/component - present in decision records, unused for scheduling.

**Recommendation.** Build the prerequisites, in this order, and let item 3
ride on them:

1. Record the features (the signal log in section 6) and start populating
   `predicted_pass` with even a base-rate model (per-component historical
   green rate). A wrong-but-recorded probability beats the current null:
   it makes mis-speculation measurable.
2. Turn the window into a graph: allow a batch whose parent is the base (a
   second root) when the probe/footprint graph says it is disjoint from the
   stack. This *is* item 3's shape, but selected by the conflict graph rather
   than by slot index - a few dozen lines in `pipeline`'s fill loop, gated by
   the same config item 3 would have added. Ship item 3 in this form as the
   stopgap; do not ship the slot-index version.
3. Order competing builds by `P_needed` once probabilities exist, and abort
   the lowest-value in-flight build when a better one queues (the planner
   move we lack). This is the step that actually reaches the paper's
   throughput, and it is a scheduler change, not a model change.

Until step 2 lands, the queue keeps the optimistic failure mode the paper
measures as worker-insensitive: one bad change near the head taxes every
batch behind it.

## 4. One conflict engine

Textual conflict knowledge currently lives twice:

- `rs/pr-landing-planner` (and its skill) builds advisory landing plans from
  file overlaps, ordering edges, and **mechanism edges** - pairs of PRs that
  touch a common recognized operational mechanism (`graph.rs`
  `build_mechanism_edges`, `mechanism.rs` vocabulary), for human review.
- The queue's own probes (`plan.rs`, `probe_cache.rs`) run merge-tree probes
  to decide "does not combine" and set changes aside; that verdict is
  operational - batches are built from it.

They already share types: the queue imports the planner's `ConflictEdge` and
`connected_components` (`plan.rs:23-24`). What is *not* shared is evidence:
the planner's mechanism/file-overlap edges never reach batch selection, and
the queue's probe outcomes (combined cleanly despite overlapping files;
failed combined despite disjoint files) never reach the planner.

Proposal: **one graph, two consumers, one ground truth.**

- Keep the merge-tree probe as the only combinability verdict. It is the only
  signal that has actually combined the trees; the planner's file-overlap
  edge is, in its own code, a conservative *assumption*
  (`build_conflict_edges_file_overlap`).
- Feed the planner's mechanism and overlap edges into the queue as the
  **prior** for `P_conf`: pairs sharing a mechanism get probed earlier,
  ordered as conflicting until a probe clears them, and counted in the
  conflict-degree rank with a distinct weight. That folds the planner's
  analysis into batch selection without giving an advisory tool a veto over
  landing.
- Feed probe outcomes back as planner input (a probed-clean pair is not
  shown to a reviewer as a conflict). The decision records already carry
  per-validation outcomes; the backfill in section 6 gives the planner the
  same file the model reads.

Do not invert this (planner verdict decides batches): the planner runs on
hosted-review data, outside the run, and its overlaps are deliberately
conservative - using them as a hard conflict would hold changes the probe
would have cleared, and would put a second, disagreeing conflict definition
on the landing path.

## 5. Binary-touching vs peripheral: measured

The consumer's validation activates all e2e tests when a change flips a bit
in the product binary, so file/test-footprint lanes cannot separate product
changes from each other. The split that separates anything is whether the
change touches an input of the binary build at all. Peripheral changes
(CI hub, scripts, docs, test-only, pin-only) can batch wide and validate
with a focused selection; binary-touching changes pay the full suite.

Measured from the copied queue history (decision records 2026-11-04/05,
113 validations; candidate diffs read from the queue's bare repository;
classifier: every changed path is CI/docs/scripts/test-only/pin or markdown):

| Quantity | Value |
|---|---|
| Batch validations | 100 |
| Unique change heads classified | 54 |
| Peripheral heads | 34 (63%) |
| Batches made only of peripheral changes | 24 (24%) |
| Share of batch validation wall time those batches burned | 36% (28,682 s of 79,461 s) |
| Median batch wall, all-peripheral vs the rest | 1,505 s vs 196 s |

Caveats: 54 heads is two days, not a season; the classifier is path-prefix
based (the real one should ask the build system whether any binary input
changed - a one-command check on the consumer side); "peripheral" batches
currently run the `all` selection, which is exactly the waste measured.

What a peripheral lane buys, on these numbers: roughly a third of validation
wall time is being spent proving that changes which cannot alter the binary
do not alter the binary's behaviour. A focused selection for all-peripheral
batches (their footprint tests only), plus wider batches for them
(they cannot conflict through the binary; textual probes still apply),
returns that third to the binary-touching queue. This composes with
section 3: peripheral-ness is also the strongest single prior for
`P_conf = 0` between two product changes and a peripheral one.

Note the interaction with the paper: Uber's workload had no such split
because every change built the same monorepo targets. Our conflict graph
should carry a `peripheral` bit per change from the start; it is the
cheapest feature in the whole design (one build-input query per head).

## 6. The per-PR signal log (refined design)

The model in section 1.2 is only as good as its history. Today the queue
records decisions per *validation*, rulings, and skips - nothing per PR
across heads, and nothing produced outside the queue (focused tests a
developer or agent ran, reviews). The signal log is that record. Build it
**after this note is agreed**; backfill makes it useful on day one.

### 6.1 Shape

- One append-only JSONL per PR in the queue state: `state/signals/<pr>.jsonl`
  (the state root is already per-repository, so no `<repo>` path segment;
  a collection of per-PR files gets its own directory, per the repository's
  layout rule). Schema `merge-queue-signal/v1`, one `at` timestamp, one
  `kind`, kind-specific fields; writers append whole lines under the item
  lock, readers tolerate a torn last line.
- Written only through the queue binary. Internal writers (the run loop,
  intake, rulings) call the same append function the CLI exposes:
  `merge-queue signal add --pr N --kind KIND --json '{...}'` for external
  producers (developer tooling, review bots, focused-test runners). No
  producer writes the files directly.
- State format stays backward compatible: the directory is new, old binaries
  ignore it, and nothing in the landing path reads it until the model does.

### 6.2 Event kinds

| Kind | Fields | Producer |
|---|---|---|
| `head` | new head, parent head, reason (`rebase`/`fix`/`review-fix`/`unknown`) | submit/resubmit intake |
| `focused-tests` | actor, head, node/test IDs, outcome per ID, wall | `signal add` (developer/agent runners) |
| `review` | reviewer, model family (for agent reviewers), verdict, head | `signal add` (review tooling) |
| `validated` | alone or batch members, candidate SHA, parent/base, verdict, failing nodes, attribution (`culprit`/`cleared`/`undetermined`) | every validation decision, from the decision record |
| `full-validate` | ledger run ID, head or ancestor SHA it covers, verdict | consumer ledger reporter |

The `validated` kind is the one the current records cannot reconstruct: a
failure in a batch is recorded against the batch. Attribution after culprit
search says `culprit` for the blamed change and `cleared` for batch-mates
that later passed; a batch-mate that was never re-run stays `undetermined`
and is still logged - "tested in conjunction with", which the owner asked
for explicitly, is a signal even when no fault attaches.

### 6.3 Backfill and export

- Backfill from `state/decisions/*.jsonl` (per-validation outcomes and the
  feature block), `state/runs/*/events.jsonl` (120 runs: plan, validate,
  result, eject, landed) and item histories: enough to synthesize `head` and
  `validated` for every PR in the copied history. Focused tests and reviews
  exist only from producers onward - the backfill must not invent them.
- Export: a daily `signal export` that appends new lines to a durable
  repository (the consumer's test ledger), keyed by PR, so the log survives
  queue-state resets and the model can train off-host. Export is a copy;
  the state files remain the live record.

### 6.4 Tie to the model's feature list

| Paper feature family | Signal log provides | Still missing |
|---|---|---|
| Change (targets, files, lines, binaries) | from decision feature block + candidate diff at backfill | affected-*target* count (needs the footprint/build graph of section 2) |
| Change: initial tests before submitting | `focused-tests` events, incl. which nodes and their outcomes | nothing, once producers emit |
| Revision (resubmissions, test plan) | `head` events count resubmissions per PR | test-plan presence (a flag a producer must assert; the queue cannot see review text) |
| Developer (name, tenure, level) | submitter identity from items, `review` reviewer/model family | tenure/level - an HR-side join the queue should not attempt; author identity is the usable proxy |
| Speculation (succeeded/failed so far) | `validated` events per head, incl. batch context and attribution | nothing - this is the family's exact data |

So the log closes every gap except target counts and developer seniority,
and both have honest substitutes (footprint overlap; author identity).
Training can start on the backfilled two days for plumbing validation, and
means something after a few weeks of live events.

## 7. What to build, in order

1. Signal log + backfill + `predicted_pass` populated by a base-rate model
   (section 6, section 3 step 1). No scheduling change.
2. Peripheral bit per change + focused selection and wider batches for
   all-peripheral batches (section 5). Independent of the model; measured
   ~36% of validation wall.
3. Disjoint second root chosen by the conflict graph (item 3 in graph form,
   section 3 step 2), default on at depth >= 2, configurable off.
4. Footprint-fed conflict prior: `ci/test-footprints.json` overlap as a
   `P_conf` prior and probe ordering (sections 2, 4).
5. Value-ordered scheduling with abort of the lowest-value in-flight build
   (section 3 step 3) - only once 1 has produced probabilities worth
   ordering by.

Items 1-2 are recording and selection changes, each landable alone. Item 5
is the only one that changes what "the window" means, and it should be
judged against the paper's optimistic-baseline numbers when it is proposed.
