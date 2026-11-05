# Retiring pr-landing-planner into a merge-queue advisory mode

Status: design note only. No code. Sources read for this note:
`rs/pr-landing-planner` (8,876 lines across 14 modules), `rs/merge-queue`
(plan, probe, decisions, signal modules), `skills/pr-landing-planner`,
`skills/pr-landing-operations`, `validation/components.json`, and the one
known consumer's CI tooling (read-only; consumers are not named here - the
integration points are described, per this repository's rule).

## 1. What pr-landing-planner does today

It is an advisory, never-mutating planner over a repository's open PRs.

**Inputs** (`plan|graph|clusters|status` verbs; `cli.rs` usage text):

- Live host collection (`host.rs`, `collect.rs`): `gh` PR views plus GraphQL
  pages for check rollups and review evidence, per-PR fetches into
  `refs/pr-landing-planner/pr-<n>`, in a caller-supplied clone.
- A deterministic fixture file instead of live data (`--fixture`), and a
  caller-owned landing-context file (`--landing-context`): validation
  evidence classified by the *caller* (the planner does not redefine what
  qualifies), the consuming workspace's `hard-green`/`soft-green` authority,
  policy classes, and assignments.
- Flaky-signature regexes (`--flaky-signatures`), a priority source
  (labels or a command), and knobs: gate check name, outage threshold,
  freshness override, batch emission, plan archives.

**Processing** (`collect.rs`, `graph.rs`, `classify.rs`, `mechanism.rs`):

- Conflicts by real `git merge-tree` probes (`--conflict-detector merge-tree`,
  measured by a consumer at ~36 ms per probe over its open set) or, as a
  conservative fallback, file overlap.
- Ordering edges from base-ref ancestry (PR-on-PR stacks), transitively
  reduced into stacks; held reasons for structurally blocked PRs.
- Mechanism edges: pairs touching a common recognized operational mechanism.
  The vocabulary is six slugs (`mechanism.rs` `Mechanism::ALL`):
  `cancel-in-progress`, `pr-auto-trigger`, `dag-scheduler-width`,
  `locally-validated-label`, `validate-ledger-path`,
  `merge-gate-required-checks`, each with label/path aliases; unclassified
  mechanism-looking changes are surfaced separately.
- CI classification per PR: red classes, and diagnostics (stale gates,
  flaky reds, real reds, evaluate-once races, runner-outage suspicion).

**Outputs** (`model.rs` `PlanResult`, `emit.rs`): the collected graph;
stacks; held PRs with reasons; the plan - `parallel_safe_groups`,
`land_now`, flattened `order`, `per_pr_actions` (action + why + group), an
optional conflict/dependency-free root `batch`; CI diagnostics; human,
JSON, and actions renderings; archived JSON plans by default on live runs.
`status` prints per-PR CI health (the verb the known consumer's health tick
actually runs, see section 2).

## 2. Who calls it

In this repository:

- `rs/merge-queue` depends on the planner **library** (`Cargo.toml`:
  `pr-landing-planner = { path = "../pr-landing-planner" }`) for exactly
  two things: the `ConflictEdge` type and `connected_components`. The
  queue's own conflict graph, probes, cache, batching, and landing path
  are otherwise entirely its own code.
- `validation/components.json` gives the planner its own component
  (`common/docs/pr-landing-planner/`, `py/bin/pr-landing-planner`,
  `py/pr_landing_planner/`, `rs/bin/pr-landing-planner`,
  `rs/pr-landing-planner/`, `skills/pr-landing-planner/`).
- `skills/pr-landing-planner` teaches agents to run it before choosing a
  landing batch; `skills/pr-landing-operations` (the execution-discipline
  skill) consumes its output; `skills/agent-utils-setup` references the pair.

In the known consumer (one, found by grepping its tree):

- Its agent-tool dispatcher offers `pr-landing-planner` (role: reporter)
  beside the queue and the DAG runner.
- Its PR-status health code has three engines; the opt-in `planner` engine
  runs the pinned planner `status` verb with real merge-tree conflicts over
  the open set every planning run (the default engine is a cheaper hosted
  read). This is the caller with the deepest integration: output fields
  feed a per-PR health board.
- Its landing guard cites the operations skill's landing-shape rules
  (screening hand-assembled waves, refused-constituent handling).
- Health checks know the `refs/pr-landing-planner/*` fetch namespace and
  exempt it from push-state complaints.

Any retirement that breaks the `status` verb's JSON shape breaks that
consumer's board on its next pin bump; that is the migration's hard edge.

## 3. Overlap with the merge queue

Following the SubmitQueue note's ruling (the queue's merge-tree probe is
the only conflict verdict; planner edges are priors), the overlap is:

| Concern | Planner | Merge queue |
|---|---|---|
| Textual conflicts | merge-tree probes or file overlap, per open PR pair | merge-tree probes (cached, incremental), per queued pair - the verdict batches are built from |
| Ordering / stacks | ancestry ordering edges, stacks | plan dependencies (ancestry + `after:`), batches in landing order |
| Batching | parallel-safe groups, root batch, ordering | rolling window batches, conflict-degree ranking, peripheral lanes, disjoint root |
| Per-PR state | hosted CI verdicts, review evidence, policy classes from caller context | queue items with full lifecycle (queued/held/ejected/landed), rulings, holds |
| Red classification | flaky/real/outage/stale-gate from hosted checks + signatures | own-validation outcomes: known-flaky, suspect reruns, decision records, signal log |
| Mechanisms | six-slug vocabulary, mechanism + unclassified edges | none (probe files + footprint prior only) |
| Review evidence | review binding per head | none (signal log records review *signals* when producers send them) |
| Priority | labels/command source | priority label + CI paths, built into plan ranking |

What is genuinely duplicated: conflict probing (two merge-tree
implementations with different caches), ordering/stacks, batching/grouping,
priority, and flaky/red classification. What the planner has that the queue
lacks: open-set host collection, mechanism vocabulary, review binding, and
caller-context evidence classification. What the queue has that the planner
lacks: everything operational (submission, validation, landing, donors,
decisions, signals).

## 4. Proposal: an advisory mode of the queue

Retire the separate tool. Provide its useful output as:

```sh
merge-queue plan --advisory [--prs N,N] [--format human|json|actions]
```

reading the queue's own plan machinery over (a) the queue's items, and
(b) - the planner's current niche - open PRs not yet submitted, collected
the way the planner collects them. Concretely:

1. **Move, don't rewrite, the graph code.** The planner's `graph.rs`,
   `mechanism.rs`, `classify.rs`, and `model.rs` graph types move into the
   merge-queue crate (the queue already compiles against two of its types;
   the dependency inverts and then disappears). The mechanism vocabulary
   moves verbatim - consumers' aliases are data, not to be redesigned.
2. **One conflict engine.** Advisory mode uses the queue's probe stage
   (`probe_with_options`, probe cache included) for conflicts - the same
   verdicts batching uses. The planner's file-overlap detector becomes what
   the SubmitQueue note already ruled it: a prior shown as such
   (`prior_conflicts`, as the footprint prior does), never a second verdict.
3. **Output shape is the planner's, mapped.** `parallel_safe_groups` from
   probe components + `partition_parallel_safe`; `land_now`/`order` from
   the queue's batch order restricted to PRs whose queue state and advisory
   evidence allow; `per_pr_actions` from queue item status where the PR is
   queued, and from collected evidence where it is not; `batch` from the
   plan's first batch; `diagnostics` from queue known-flaky/suspect data
   plus hosted red classification where collected. A `status --advisory`
   form (or the same verb with `--format json`) reproduces the planner
   `status` fields the consumer's board reads - names and shapes pinned by
   a fixture test taken from the planner's own tests.
4. **Inputs stay caller-owned.** `--landing-context` and
   `--flaky-signatures` keep their formats; evidence classification stays
   the caller's (`hard-green`/`soft-green` is not reconstructed). Open-set
   collection moves with the graph code, keeping `refs/pr-landing-planner/*`
   as the fetch namespace during migration (rename only with the removal).
5. **No landing.** Advisory mode takes no writer lock beyond planning's,
   writes nothing but the existing plan archive equivalent, and says so.

What this deliberately does not do: fold the planner's hosted-CI
*collection cadence* into the queue daemon. Advisory is an invocation, not
a daemon duty; a health tick that wants it runs the command, as it runs
the planner today.

## 5. Migration and deprecation

1. Land advisory mode behind `--advisory` (default plan output unchanged),
   with the planner's fixture tests ported as the compatibility contract
   (same fixture in, same groups/actions out, modulo field provenance).
2. Ship a `pr-landing-planner` shim in the same release: the old binary
   name forwards `plan|graph|clusters|status` to `merge-queue plan
   --advisory` equivalents (and `quickstart`/`--userguide` print a pointer),
   warning once on stderr. JSON consumers see the same last-line JSON.
3. Rewrite `skills/pr-landing-planner` to invoke the advisory mode;
   `skills/pr-landing-operations` keeps its text, changing only which
   command produces the plan it consumes (see section 6).
4. The consumer migrates its health board's planner engine to
   `merge-queue plan --advisory --format json` (or the shim) on its next
   pin bump; the field-pinning fixture test is the contract it bumps
   against. Its `refs/pr-landing-planner/*` namespace stays valid through
   the shim period.
5. Removal: after one full release cycle with the shim (and at least one
   consumer pin bump past it), delete `rs/pr-landing-planner`, the Python
   wrapper, the component entry (folded into the merge-queue component),
   and the shim. The mechanism vocabulary and graph code live on inside
   the queue; nothing else references the crate (the queue's `Cargo.toml`
   dependency goes away in step 1, not step 5).

Risks, honestly: the planner's collection is `gh`-heavy and its latency
profile (a batched fetch after a consumer's own optimization, seconds over
an open set) becomes the queue binary's problem in advisory invocations;
fixture-mode parity must be proven, not asserted, before the shim ships;
and two renderings (human/actions) have operator muscle memory - keep
their shapes through the shim period.

## 6. pr-landing-operations afterwards

Yes, it still makes sense afterwards: its subject is execution discipline
under a consuming repository's own authorization rules (exact-revision
evidence, screened coalesced landings, serialized publication), which is
orthogonal to which binary produced the advisory plan - it should keep
existing, pointing at `merge-queue plan --advisory` output instead of the
planner's.
