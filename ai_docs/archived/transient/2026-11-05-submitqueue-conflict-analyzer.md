# Conflict analyzer for the merge queue: what "independent" may mean on a gated main

Status: DESIGN NOTE ONLY, for owner review. No code, nothing enabled. Written
against `rs/merge-queue` on agent-utils `main` at `fea446e5` (P1 predictor)
plus the k-roots landing `05f35064`, and the paper: Ananthanarayanan et al.,
"Keeping Master Green at Scale", EuroSys '19, §5 (copy read via the design
note `2026-11-05-submitqueue-throughput-ordering.md` in this directory).
The consumer is referred to generically throughout.

## 1. The question

The queue may validate changes in parallel (stacked speculation, disjoint
roots) but lands them one exact validated tree at a time. The conflict
analyzer answers a narrower question than "do these changes conflict":

> When change B was validated on base X, and change A has since landed
> (moving the base to X+A), under what conditions may B land without a new
> validation of X+A+B?

Today the answer is never: any base movement invalidates the tree, and B is
rebuilt and revalidated (or kept, under `keep_stale_roots`, only to be
rebuilt). That is sound and expensive. §5 of the paper buys back the cost
by proving most pairs independent *before* validation, so their order does
not matter. This note defines what proof we can honestly offer at each
level of evidence, and the rule that keeps a gated main green.

## 2. The soundness rule (the whole proposal)

Two changes A and B are **independent** only if all of the following hold:

1. **Textual**: `git merge-tree` combines A and B cleanly in both orders
   (the existing probe verdict; necessary, never sufficient alone).
2. **Affected-set disjoint**: their affected sets (defined per level in §3)
   do not intersect. Unknown affected sets are *not* disjoint: any change
   whose affected set cannot be computed counts as affecting everything.
3. **No shared mechanism**: neither pair shares a mechanism edge (the
   landing planner's operational-mechanism vocabulary, e.g. both sides
   rewiring the same generated-count or pin mechanism) — mechanisms are
   how disjoint files still compose into one behaviour.
4. **Composition check**: the combined tree's affected-set hash equals the
   union predicted from the single-change graphs (the paper's Eq. 6 idea);
   if the build system cannot produce that hash, this clause is
   *unsatisfied*, not waived.

And the landing rule that makes independence safe to use:

5. **Landing still checks the tree.** B lands without revalidation only
   when the tree of X+A+B is byte-identical to a tree already validated
   green (exact-tree reuse, the queue's `exact_sha` discipline), or when
   A and B are independent by 1–4 *and* the validation that passed for B
   covered every test in B's affected set that A's landing could alter
   (affected-set coverage, §4). Otherwise B revalidates. Independence
   never lets an unvalidated tree land; it only lets a validated tree
   survive an unrelated landing.

Clause 5 is the gated-main invariant restated: main only ever advances to
an exact validated tree, or to a tree whose difference from a validated
tree is confined to an affected set the validation provably covered and
the landed change provably does not touch.

## 3. Levels of affected set, weakest to strongest

| Level | Affected set of a change | Sound? | Cost | Status today |
|---|---|---|---|---|
| Files | changed paths | No: disjoint files routinely share one binary/test (the design note §2 measured this) | trivial | exists (probes) |
| Test footprints | tests whose footprint (`ci/test-footprints.json`: test -> files it reads) intersects changed files, closed over the tests themselves | Sound for *test outcomes* iff footprints are complete; unsound where a test reads undeclared inputs | one join per change | raw data exists consumer-side; queue reads it only as a probe prior (`conflict_prior.footprints`) |
| Components/crates | workspace crates (or first-directory components) touched, closed over reverse dependencies | Sound at crate granularity iff the dependency closure is complete; misses runtime/data coupling | needs the workspace graph | components (first dir) recorded; no closure |
| Build/test DAG nodes | build targets whose content hash changes, closed over dependents (paper §5, Algorithm 1) | The paper's level: sound modulo hash completeness | n graphs (union-graph approximation, §5) | not available: no build graph is exposed to the queue |
| Peripheral split | whether any input of the product binary changed (the consumer hook) | Orthogonal bit, not a level: a peripheral change and a binary-touching change can still share scripts/tests | one hook call | exists (`peripheral` lane) |

Rules per level:

- The analyzer uses the **strongest level available per pair**, and says
  which level decided. A pair decided at file level is labelled
  `files-only` and is *not* eligible for clause-5 reuse, only for probe
  ordering (exactly today's footprint-prior role).
- Footprint level is the strongest we can build now. Its soundness rests
  entirely on footprint completeness, so the analyzer must treat a test
  with no footprint entry as affected-by-everything, and a change touching
  a file no footprint names as affecting the full suite selection.
- DAG-node level is the only level at which clause 4 (composition hash)
  can be checked. Until a build graph exists, clause-5 reuse is limited
  to the exact-tree case plus the footprint-coverage case, and the note
  recommends the owner treat that as the v1 boundary.

## 4. Affected-set coverage (the footprint-level landing rule)

B validated on X with selection S (the tests actually run). A lands.
B may land on X+A without revalidation iff:

- A and B are independent at footprint level or stronger (§2.1–2.3), and
- every test in `affected(B)` that is also in `affected(A)`'s test closure
  is in S — vacuously true when the closures are disjoint — and
- B's validation was `all` (complete selection), or `affected(B) ⊆ S`
  (the focused selection covered B's whole affected set; this composes
  with the queue's existing widening obligations, which still apply to
  the *landed* tree exactly as today), and
- neither A nor B is graph-changing at the available level (footprint
  file edited, build file edited, mechanism file edited): graph-changing
  changes invalidate every affected set computed from the old graph, so
  they conflict with everything validated against it (the paper's 7.9% /
  1.6% observation is what makes this affordable).

## 5. The union-graph approximation, honestly scoped

The paper avoids n² build graphs: build HEAD, HEAD+Ci, HEAD+Cj (n graphs),
propagate "affected" marks topologically on the union graph; a target
marked by both changes conflicts. Our analogue without a build graph:

- Per change, one footprint join against the base's footprint file gives
  `affected(Ci)`; pairwise intersection is then set algebra, no extra
  builds. This is *cheaper* than the paper's approximation and *weaker*:
  it cannot see a target that both changes affect only in composition
  (clause 4 unsatisfied), which is why §4 keeps the exact-tree check and
  the graph-changing exclusion as compensating controls.
- If a build graph ever becomes available, replace the join with the
  union-graph propagation unchanged: the interface (per-change affected
  set + graph-changing bit) is the same.

## 6. What this buys, and what it does not

- Buys: rooted validations stop being wasted when an unrelated change
  lands first (today: cancelled or kept-then-rebuilt). With k roots this
  is the difference between roots as latency insurance and roots as
  throughput. It also sharpens P2: `P_conf` priors get a structural
  zero for footprint-disjoint pairs instead of only learned rates.
- Does not buy: permission to skip validation, parallel landing, or any
  weakening of the exact-tree rule. Main still moves one validated tree
  at a time; a wrong independence verdict degrades to a revalidation
  that clause 5 forces whenever coverage cannot be shown, and to a red
  main only if footprint data is *wrong* — which is why completeness
  (unknown = affected-by-everything) is fail-closed throughout.

## 7. Proposed implementation shape (after approval only)

1. `conflict.rs` (new module): affected-set computation at footprint
   level from the existing `conflict_prior.footprints` data plus the
   plan's probe results; per-pair verdict with its level recorded.
2. Decision records gain the pair level used; `stats` reports how often
   each level decided and how often reuse was taken vs forced revalidation.
3. Run loop: clause-5 reuse behind a new default-off config flag
   (`conflict_reuse`), applied only to rooted flights whose base moved;
   every reuse logged with both changes, the level, and the coverage
   witness. Off = today's behaviour byte-for-byte.
4. Tests: footprint-disjoint pair reuses; footprint-overlapping pair
   revalidates; unknown-footprint change revalidates; graph-changing
   change revalidates everything behind it; exact-tree case unchanged.

## 8. Decisions asked of the owner

1. Is footprint-level independence (§4), with its completeness
   assumptions, an acceptable basis for clause-5 reuse, or must v1 be
   exact-tree only?
2. Should graph-changing be defined by path rules the queue configures,
   or by the footprint/build system's own declaration?
3. May the analyzer consume the landing planner's mechanism edges as a
   hard independence blocker (§2.3), or only as a probe-ordering prior
   (the earlier design note's recommendation)?
