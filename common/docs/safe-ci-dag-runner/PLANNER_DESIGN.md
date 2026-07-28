# Planner design: a measured-curve moldable allocator for safe-ci-dag-runner

Status: IMPLEMENTED in v0.8.0 as `--planner cpa` (this document is both the design rationale and
the specification the implementation follows). It grounds the choice of algorithm in the scheduling
literature, then specifies the algorithm precisely enough that the Python and Rust builds implement
it identically (the cross-differential compares the resulting plan — chosen widths, order, makespan
lower bound + modeled makespan, and stop reason — byte-for-byte). Everything the allocator consumes
(measured speedup curves, the critical-path list scheduler, the memory and named-resource models)
shipped in v0.7.0; the allocator that decides each step's inner-parallelism width, the runtime
core-budget dispatch gate, and the modeled-makespan reporting are the v0.8.0 additions. Section 5
below is the authoritative spec; where the shipped code refines a detail, the section notes it.

## 1. The problem, in plain language

We run a directed acyclic graph (DAG) of CI/build steps on **one machine** with a fixed budget of
`P` CPU cores (a cgroup CPU quota, or a CPU-affinity width). Steps are connected by **precedence**
edges: a step cannot start until every step it depends on has finished successfully.

Each step is **moldable**: we can hand it a number of inner worker threads `p` (its `-j` value —
e.g. `make -j p`, `cargo test -j p`, `pytest -n p`) *once, at launch*, and its wall-clock time then
follows a **measured** curve `T_i(p)` — the wall time of step *i* as a function of the cores we give
it. "Moldable" (fix the width at launch) is the middle of the three standard classes:

- **rigid** — the width is fixed in advance and the scheduler cannot change it;
- **moldable** — the scheduler picks the width once, at launch, and it stays fixed for the run;
- **malleable** — the width can change *while the step is running*.

We are moldable: a build's `-j` is chosen when we spawn it and does not change mid-flight.

The goal is to **minimize the makespan** — the wall-clock time from the first step starting to the
last step finishing — subject to four constraints that all coexist on the one machine:

1. **Precedence.** The DAG edges (`Step.deps`).
2. **A total core budget `P`.** The sum of the widths of the steps running *at the same instant*
   should not oversubscribe the machine. `P` comes from `container_core_budget()` (the cgroup
   `cpu.max` quota ÷ period, else `nproc`).
3. **Per-step memory caps.** Each step has a modeled peak memory footprint; the worst-case sum over
   any set of steps that can co-run must fit a RAM budget. This is the existing reachable-concurrent-set
   memory model in `sizing.py` (`schedulable_peak_mem_bytes`), and a CPU-bound step's cap *grows* with
   its width (`step_mem_cap_for_inner_jobs`), so **widening a step costs memory**.
4. **Named scarce-resource semaphores.** Arbitrary caller-named capacities in `DagConfig.resource_caps`
   (e.g. `{"browser": 1}`) bound how many demanders co-run — which in turn bounds the *achievable*
   concurrency and therefore how much of `P` can actually be kept busy.

What v0.7.0 already has:

- **The measured curves.** `estimates.load_step_speedups()` reads the profile store and produces, per
  step, a `StepSpeedup` with one `SpeedupLevel` per measured width — carrying the robust
  (contention-discounted, outlier-trimmed) `wall_s`, the **total CPU-seconds** `cpu_s = user_s + sys_s`
  (a *work-conservation* signal), `effective_cores`, `throttled_s`, and the `speedup` versus the
  smallest measured width. It also computes a **per-step** `recommended_inner_jobs` — the best width
  before that one step's own diminishing-returns knee.
- **The list scheduler.** `scheduler.py`'s `Runner` is a greedy ready-set list scheduler: it launches
  every ready step (deps met, memory + named resources free, under `-j`) in a caller-supplied
  dispatch order, and `--planner critical-path` supplies a *critical-path-first* order (highest
  bottom-level first). This is exactly the "list-scheduling" back half of a two-phase moldable
  scheduler.
- **The memory + resource models.** As above.

**The missing piece — this document's subject — is the ALLOCATOR**: the phase that picks each step's
width `p_i` to minimize whole-DAG makespan, *then* hands those widths (and the resulting
allocation-aware ordering) to the existing list scheduler. Today `p_i` is either a static DAG hint or
each step's *isolated* per-step knee; neither reasons about the DAG as a whole or about steps sharing
the one machine.

## 2. The makespan lower bound that drives everything

For a DAG scheduled on `P` processors, any schedule's makespan `T` obeys two independent lower
bounds (Graham 1969; Brent 1974):

- **The critical-path (span) bound.** `T >= T_CP`, where `T_CP` is the length of the longest chain of
  dependent steps, each weighted by its chosen wall time `T_i(p_i)`. You cannot finish faster than
  the longest dependency chain, no matter how many cores you have.
- **The work/area bound.** `T >= A / P`, where the **area** `A = sum_i p_i * T_i(p_i)` is the total
  core-seconds of work at the chosen widths, and `P` is the core budget. You cannot finish faster
  than the total work spread perfectly across `P` cores.

So `T >= max(T_CP, A/P)`. Brent's theorem is the matching upper direction: a greedy (list) schedule
achieves `T <= T_1/P + T_inf` — within a factor ~2 of the max of the two terms (Graham's `2 - 1/P`
list-scheduling guarantee is the same phenomenon).

This inequality is the *entire* reason a moldable allocator is subtle, and why the recommended
algorithm has the shape it does:

- Widening one step (raising `p_i`) **shrinks `T_i(p_i)`**, which can shorten `T_CP` — good, *if that
  step is on the critical path*.
- But widening **grows the area** `A` (you spend `p_i * T_i(p_i)` core-seconds, and real curves are
  sub-linear, so `p_i * T_i(p_i)` *rises* with `p_i`), which **raises `A/P`** — bad.

The two terms pull in opposite directions. The optimum of the two-term bound sits where they
**balance**: `T_CP ~= A/P`. Below that point the critical path dominates and you should spend cores to
shorten it; above it the area dominates and extra cores are wasted. An allocator that walks the
allocation toward `T_CP ~= A/P` is directly minimizing the quantity that lower-bounds (and, via
Graham/Brent, upper-bounds within ~2x) the makespan. This is precisely what CPA does.

## 3. Related work

### 3.1 Speedup models: analytic vs. measured

Classic moldable scheduling assumes an *analytic* speedup function `T_i(p)`:

- **Amdahl (1967)** — a fixed serial fraction caps speedup; `T(p) = T(1)·(s + (1-s)/p)`. Pessimistic
  for CI steps whose serial fraction is small but real (link steps, test-runner startup).
- **Downey (1997), "A model for speedup of parallel programs" (UC Berkeley TR CSD-97-933)** — a
  two-parameter model (average parallelism `A` and a variance `sigma`) giving a smooth, concave,
  saturating speedup curve. Downey's model is the canonical *parametric* stand-in when you have no
  measurements: near-linear up to `~A`, then a knee, then a plateau.

**We do not need to assume a model — we measure the curve.** `load_step_speedups()` gives `T_i(p)` as
a set of real, robust, contention-discounted points at the widths we have actually run
(`SpeedupLevel.wall_s`), plus the total-work signal `cpu_s`. The literature's analytic models exist
to *approximate* exactly the object we have empirically. This is our single biggest deviation from
the textbook (see §6) and it is a strength: the concavity, the knee location, and the
work-conservation break are observed, not assumed. Amdahl/Downey remain useful as (a) a sanity check
on the shape of a measured curve and (b) a fallback prior for a step with too few samples to have a
measured curve.

### 3.2 The area/critical-path allocation lineage

- **Belkhale & Banerjee (1990)** introduced allocating processors to *independent* moldable tasks by
  balancing per-task area, an early "area-aware" allotment heuristic.
- **Turek, Wolf & Yu (1992, SPAA), "Approximate algorithms for scheduling parallelizable tasks"** and
  **Ludwig & Tiwari (1994, SODA), "Scheduling malleable and nonmalleable parallel tasks"** established
  the **two-phase paradigm** for *independent* malleable tasks: **phase 1** chooses an *allotment* (a
  width per task), **phase 2** *list-schedules* the now-rigid tasks. Turek-Wolf-Yu give a 2-approximation
  by searching over a candidate makespan and solving the induced allotment; Ludwig-Tiwari improve the
  allotment step (a `(2+eps)` / practical improvement) using the tasks' *work* functions. The
  structural lesson — **allot, then list-schedule** — is exactly the architecture we adopt. The
  caveat: their approximation guarantees are for **independent** tasks (no precedence). Our DAG has
  precedence, so we borrow the *structure*, not the bound.
- **Radulescu & van Gemund (2001, ICPP), "A Low-Cost Approach towards Mixed Task and Data Parallel
  Scheduling" — CPA.** CPA brings the two-phase idea to *precedence-constrained* DAGs with a cheap,
  greedy allocation phase built directly on the `max(T_CP, A/P)` bound above. Its allocation phase:
  start every task at one processor; while the critical-path length exceeds the average area per
  processor (`T_CP > T_A`, with `T_A = A/P`), pick the task **on the current critical path** whose
  extra processor helps most and give it one more; recompute; repeat until `T_CP <= T_A`. Then
  list-schedule. CPA is O(cheap) and needs no makespan guess — it just walks to the balance point of
  the two-term bound. **This is the closest published match to our problem and the basis of our
  recommendation.**
- **Bansal, Kumar & Singh (2006, Parallel Computing), "An improved two-step algorithm for task and
  data parallel scheduling in distributed memory machines" — MCPA (Modified CPA).** MCPA fixes CPA's
  main blind spot: CPA can over-allocate a critical-path task even when its *siblings* (tasks that
  will run at the same time) could have used those processors concurrently. MCPA makes the allocation
  **level/concurrency aware** — it limits a task's width by how many tasks are actually runnable
  alongside it. On a *small-`P` single machine*, where oversubscription directly causes CPU throttling,
  this insight matters, and we fold a bounded form of it into our core-budget gate (§5).

### 3.3 Approximation theory for the precedence case

- **Jansen & Zhang (2005 ISAAC / 2006 ACM TALG), "An approximation algorithm for scheduling malleable
  tasks under general precedence constraints"** give a constant-factor approximation (`~4.73`, improved
  in later work) for *exactly* our theoretical problem — malleable tasks with a general DAG. Together
  with **Jansen (2004, Algorithmica), the malleable-task FPTAS/AFPTAS line**, these results are the
  theoretical backdrop that *validates* optimizing the two-term `max(T_CP, A/P)` objective: they show
  it is the right quantity and that constant-factor guarantees are attainable. They are, however, LP-
  or dual-approximation-based, heavier than we need for CI DAGs of tens of steps, and they assume
  analytic monotone work functions. We treat them as justification, not as the implementation.

### 3.4 List-scheduling ORDER heuristics (phase 2)

The allocation phase decides *widths*; the list-scheduling phase decides *order among ready steps*.
The order literature is where our existing `--planner` lives:

- **HLFET (Adam, Chandy & Dickson, 1974, CACM), "A comparison of list schedules for parallel
  processing systems"** — Highest Level First with Estimated Times: order ready tasks by their
  **static bottom-level** (longest remaining weighted path to a sink). This is *exactly* our
  `--planner critical-path` (`estimates._bottom_levels` / `_plan_order`).
- **HEFT (Topcuoglu, Hariri & Wu, 2002, IEEE TPDS 13(3)), "Performance-Effective and Low-Complexity
  Task Scheduling for Heterogeneous Computing"** — upward-rank ordering plus an earliest-finish-time
  processor choice. HEFT is the most-cited DAG scheduler, **but it targets HETEROGENEOUS processors**:
  its cleverness is choosing *which* (different-speed) processor runs each task. **We are homogeneous**
  — one pool of identical cores — so HEFT's processor-selection half is moot for us; its ordering half
  reduces to the same bottom-level/upward-rank idea we already use. We cite HEFT to explain *why we do
  not adopt it wholesale*: its core contribution does not apply to a single homogeneous machine.

## 4. Recommended algorithm

**Adopt a CPA-style critical-path / area-balancing moldable allocator, driven by our measured
`T_i(p)` curves, feeding the existing critical-path list scheduler, with the memory caps, named-
resource semaphores, and the total core budget `P` as hard constraints on allocation.**

Concretely, the next planner is a **two-phase** design (Turek-Wolf-Yu / Ludwig-Tiwari structure):

- **Phase 1 — allocation (new):** a CPA/MCPA-derived gradient loop picks each step's width `p_i`,
  snapping to the widths we have actually measured, balancing `T_CP` against `A/P` and stopping at
  each step's work-conservation knee or when a constraint binds.
- **Phase 2 — list scheduling (exists):** recompute bottom-levels using the *allocated* weights
  `T_i(p_i)`, order ready steps by bottom-level (our `critical-path` planner), and dispatch under the
  memory, named-resource, and (new) core-budget gates.

### Why this over the alternatives

- **vs. v0.7.0's per-step greedy-knee.** The current `recommended_inner_jobs` optimizes each step *in
  isolation* — best wall before *its own* knee, capped at `P`. That is a fine default for one step but
  is **DAG-blind and machine-blind**: it will happily push a plateau-ish step toward `P` even when the
  step is off the critical path (those cores buy no makespan) and even when three such steps co-run
  and collectively demand `3P` cores. The CPA allocator spends cores where they shorten the *whole-DAG*
  critical path and refuses to oversubscribe the shared machine. (The per-step knee stays as the
  fallback for steps with no usable curve, and as the per-step *upper bound* the allocator never
  exceeds — see §5.)
- **vs. pure two-phase (Turek-Wolf-Yu / Ludwig-Tiwari).** Their guarantees assume **independent**
  tasks; our DAG has precedence, so the bound does not transfer, and their allotment search (binary-
  search a makespan, solve a knapsack-like allotment) is heavier and assumes analytic work functions.
  CPA gives us the same allot-then-schedule structure with a **precedence-aware**, guess-free,
  O(cheap) allocation loop that reads straight off our measured curves. We keep their architecture and
  drop their independence assumption.
- **vs. MCPA in full.** MCPA's concurrency-awareness is genuinely useful at small `P`, but the full
  level-by-level machinery adds state and is harder to make bit-for-bit deterministic across two
  languages. We take its **key insight** — never allocate more cores to a step than can be used given
  what co-runs — via the core-budget gate and a concurrency-aware per-step cap, while keeping CPA's
  simpler global CP/area loop as the deterministic base. Full MCPA level-balancing is a noted future
  refinement.
- **vs. Jansen-Zhang / Jansen FPTAS.** Constant-factor-optimal but LP-based, complex, and reliant on
  analytic monotone work functions — overkill for tens-of-steps CI DAGs and hard to make deterministic
  and dependency-light. We cite them as the theory that says our objective is the right one.

## 5. Algorithm specification

This section is precise enough to implement identically in `py/` and `rs/`. All arithmetic and all
tie-breaks are specified so the two builds agree bit-for-bit (the cross-differential compares plan
output byte-for-byte).

### 5.1 Inputs

- `cfg: DagConfig` — the DAG, `resource_caps`, and memory tunables.
- `speedups: Mapping[tag, StepSpeedup]` — measured curves from `load_step_speedups()`. Each carries
  ascending `levels` with `inner_jobs`, `wall_s`, `cpu_s` (may be `None`), and the per-level
  `speedup`.
- `est: Mapping[tag, float]` — the resolved scalar duration per step (store-over-hint-over-default),
  as `build_plan` already computes. Used as the wall time for any step **without** a measured curve.
- `P: int` — the core budget, `container_core_budget()`.
- `mem_budget: int | None` — the RAM budget (as `--max-mem` already supplies to `jobs_for_budget`);
  `None` disables the memory constraint on allocation.

### 5.2 Per-step admissible widths and the measured wall function

For each step `i`, define its **admissible width set** `W_i`:

- If `i` has a measured curve, `W_i = { level.inner_jobs for level in speedup.levels }`, but
  **truncated at the step's work-conservation knee** exactly as `_build_step_speedup` already computes
  it: never admit a width beyond `speedup.recommended_inner_jobs`. This reuses the existing knee
  (marginal wall gain `>= 1.15x` AND total-CPU-seconds growth `<= 1.5x` AND `<= P`) so the allocator
  cannot push a step past the point where extra cores stop conserving work. Also drop any width `> P`.
- If `i` has **no** measured curve, `W_i = { w_i }` where `w_i` is the step's `preferred_inner_jobs`
  hint (or `1`). The step is effectively rigid; the allocator will not widen it (no data to justify
  it).

Define the **measured wall** `T_i(p)` for `p in W_i` as that level's `wall_s`; for a curveless step,
`T_i(w_i) = est[i]`. `T_i` is only ever evaluated at admissible widths — **no interpolation or
extrapolation**, so allocations always sit on real measurements (a deliberate deviation, §6).

Sort each `W_i` ascending. Let `p_i` be the step's *current* allocation (an index/value into `W_i`);
let `next(p_i)` be the next-larger admissible width, or `None` if `p_i` is already the largest.

### 5.3 Seed the initial allocation

Set `p_i := min(W_i)` for every step — i.e. the **narrowest** admissible width (usually 1). This is
CPA's "start everyone at one processor" seed. Rationale: starting minimal and *growing* toward the
balance point makes the loop monotone (area only rises) and the stop condition well-defined; it also
makes the result independent of any prior allocation state (determinism).

### 5.4 Derived quantities (recomputed each iteration)

- **Weighted bottom-levels.** Run the existing `_bottom_levels` with the *current* weights
  `w(i) = T_i(p_i)`. This yields, for every step, the longest remaining dependency chain in wall time
  at the current allocation.
- **Critical path and its length `T_CP`.** The existing `_critical_path` over those bottom-levels
  (start at the max bottom-level, follow max-bottom-level successors; ties by tag ascending). `T_CP`
  is the length; `CP` is the set of steps on it.
- **Area and the area term.** `A = sum_i p_i * T_i(p_i)`; `T_A = A / P`.

### 5.5 The gradient step (which step to widen, and by how much)

While `T_CP > T_A` **and** at least one critical-path step can still be widened within all
constraints:

1. **Candidate set.** Consider only steps `i in CP` with `next(p_i) != None` (a wider admissible
   width exists).
2. **Marginal gain (the selection metric).** For each candidate, using the *measured* curve, compute
   the **actual** wall reduction from the next width and the cores it costs:

   ```
   delta_wall(i) = T_i(p_i) - T_i(next(p_i))          # measured, >= 0 (knee-truncated => monotone)
   delta_cores(i) = next(p_i) - p_i                    # > 0
   gain(i) = delta_wall(i) / delta_cores(i)            # wall reduction per added core
   ```

   `gain` is the reduction-per-added-core. It uses the real measured drop, which is *strictly more
   accurate* than CPA's original `T_i(p_i)/p_i` proxy (that proxy assumes near-linear speedup; we have
   the curve, so we use it).
3. **Constraint filter — a candidate is admissible only if widening it keeps every constraint
   satisfied** (see §5.6). If widening `i` would violate a constraint, `i` is dropped from the
   candidate set this iteration (it may still be widened later if freeing elsewhere makes room — but
   because area is monotone increasing, in practice a violated core/memory constraint stays violated,
   which is what makes the loop terminate).
4. **Pick and apply.** Choose the admissible candidate with the **greatest `gain`**; break ties by
   **smallest tag** (ascending, matching the existing critical-path tie-break). Set `p_i := next(p_i)`.
5. **Recompute** §5.4 and loop.

### 5.6 How the constraints bind the allocation

A tentative widening `p_i -> next(p_i)` is rejected unless **all** hold:

- **Total core budget.** The step's own width must not exceed `P`: `next(p_i) <= P`. (Cross-step
  oversubscription is handled at dispatch, §5.7, but no single step may exceed the whole machine.)
- **Per-step memory cap grows with width.** Recompute the DAG's worst-case concurrent footprint with
  the tentative width using the existing `schedulable_peak_mem_bytes` /
  `step_mem_cap_for_inner_jobs` (a CPU-bound step's cap scales `~ cap * p / 4`). Reject if the new
  worst-case footprint exceeds `mem_budget`. This is the point where "widening costs memory" enters:
  a CPU-bound step on the critical path may be *memory-blocked* from widening even though cores are
  free. (v0.8.0 note: the shipped allocator reads each step's DAG-declared `rss_baseline_bytes` for
  this check — the same baselines `sizing.py` always consumed. `mem_budget` comes from `--max-mem`;
  absent, the memory constraint is off. Feeding store-*learned* peak baselines into the allocation
  memory model is a scoped follow-on.)
- **Named-resource feasibility is unchanged by width** (widths do not change `hint.resources`), so
  named-resource caps never *block a widening* — but they *do* shape the area term's realizability:
  see §5.7.
- **Knee already enforced** by `W_i` truncation (§5.2): the work-conservation guard cannot be
  exceeded because those widths are not admissible in the first place.

### 5.7 Handing allotments to the list scheduler (phase 2)

The allocator outputs `p_i` per step. Wiring into the existing runner:

1. **Bake widths in.** Produce a `DagConfig` copy whose each `Step.hint.preferred_inner_jobs = p_i`
   (analogous to `apply_plan_to_config`). This flows through `command_with_inner_jobs` (the `-j`
   flag actually handed to the step) and through `step_mem_cap_for_inner_jobs` (the memory cap).
2. **Order.** Compute the dispatch order with `--planner critical-path` using the *allocated* weights
   `T_i(p_i)` (not the p=1 or hint weights). Highest weighted bottom-level first, ties by tag.
3. **Add a core-budget gate to the runner (new, MCPA's insight).** Introduce cores as a first-class
   capacity dimension alongside the named-resource semaphores: treat `P` as a built-in `"cpu"` cap and
   each step's demand as `p_i`. The ready-set loop then launches a ready step only if
   `sum(p_j for j running) + p_i <= P`, exactly mirroring `_res_free` / `_acquire` / `_release`. This
   closes the loop between the allocator's `A/P` assumption and runtime: the machine is never
   oversubscribed, so measured curves (gathered boxed, one step's cores to itself) stay predictive.
   Named-resource caps continue to gate co-running independently; together they bound the achievable
   concurrency the area term assumes.

The result: cores flow to the steps that shorten the whole-DAG critical path and *scale* (per the
measured curve), steps that plateau keep their cores small, memory-heavy steps are throttled by the
RAM budget, and the machine is never oversubscribed.

### 5.8 Stop conditions (any one ends the loop)

- **Balance reached:** `T_CP <= T_A`. The critical path no longer dominates; further widening only
  grows area. (CPA's native termination.) The shipped code reports this stop reason as `balanced`.
- **No admissible candidate:** every critical-path step is already at its knee-truncated max width, at
  `P`, or blocked by the memory budget. Additional cores cannot help the current critical path. The
  shipped code distinguishes three sub-cases in the reported stop reason: `knee-exhausted` (no
  critical-path step has any wider admissible width left — including the per-step `P` cap, which is
  enforced by construction via the `W_i` truncation in §5.2), and `mem-capped` / `core-capped` (a
  wider width exists but every candidate was rejected by the memory budget / the core cap this
  iteration). Because §5.2 truncates `W_i` to widths `<= P`, `core-capped` is a defensive backstop
  that does not arise in practice; the P-cap surfaces as `knee-exhausted`.
- **Fixed-point safety net:** if an iteration applies no change, stop (reported `fixed-point`). Guards
  against any tie/rounding corner making the loop spin.

Because every applied step strictly increases some `p_i` within a finite `W_i`, and area is monotone
non-decreasing, the loop runs at most `sum_i (|W_i| - 1)` iterations — trivially bounded for CI DAGs.

### 5.9 Determinism (Python/Rust bit-for-bit parity)

The cross-differential (`cross/differential.py`) compares plan output byte-for-byte, so the allocator
must be deterministic and identical across builds:

- **Compare gains at fixed precision.** Compute `gain(i)` and compare the `T_CP > T_A` condition using
  the SAME fixed-precision convention the plan JSON already uses (`_fmt_secs` — fixed 3 decimals): reduce
  each compared quantity to its rounded-to-3-decimals form (or scaled-integer millis) before
  comparing, so no float-representation difference between languages can flip a `>` or a tie. This
  mirrors the existing `estimates.py` discipline (integer-rank percentiles, fixed-precision JSON).
- **Total tie-break order.** When two candidates have equal rounded `gain`, pick the smallest tag
  (ascending). Reuse the existing `_neg_key` / `(-bottom_level, tag)` convention so ordering is total
  and matches the critical-path planner.
- **Canonical iteration order.** Iterate steps in `cfg` registration order when building any
  intermediate list, so accumulation order (and thus any floating add order for `A`) is identical
  across builds.
- **Integer core arithmetic.** `delta_cores`, `P`, and the core-budget gate are pure integers.

**Realized mechanism (v0.8.0).** The shipped allocator meets the bit-for-bit requirement the same
way the rest of this codebase does: it performs the SAME floating-point operations in the SAME
canonical order (cfg registration order for the area sum; the shared `_bottom_levels` /
`_critical_path` for the weighted CP; a tag-ascending scan that keeps the first maximum for the
gradient tie-break) in both languages, so every `>`/`<=`/max/min comparison sees bit-identical IEEE-754
values and cannot diverge. Widths, `P`, and the core-budget gate are pure integer arithmetic. The
cross-differential (`compare_cpa_planner`) proves the resulting plan JSON/text — including the chosen
widths, the area/critical-path/lower-bound/modeled-makespan numbers (emitted as fixed-3-decimal
strings via `_fmt_secs`), and the stop reason — is byte-identical py-vs-rs. (Pre-rounding each
compared quantity to 3 decimals, floated as an alternative above, proved unnecessary given identical
op order; the differential is the guarantee.)

The modeled makespan reported alongside the lower bound is a deterministic greedy list-schedule of
the allocated widths (§5.7's phase-2 dispatch, simulated): it respects the DAG dependencies AND the
core budget, so it is provably `>=` the `max(T_CP, area/P)` lower bound, and the plan asserts this.

Expose the chosen widths and the loop's stopping reason in `plan`/`--show-plan` output (a new column
`alloc_inner_jobs` and a one-line `allocator (cpa): <stop-reason>; P=<N> cores; critical-path=…s,
area/P=…s, lower-bound=…s, modeled-makespan=…s`), plus a machine-readable `allocation` object in the
plan JSON, so an operator (or a CI-optimizing agent) can see *why* each width was chosen — matching
the existing "show the estimate source" philosophy.

## 6. Our deviations from the textbook

1. **Empirical curves, not analytic models.** CPA/MCPA/Jansen assume an analytic `T_i(p)` (Amdahl- or
   Downey-shaped, monotone, concave). We read `T_i(p)` from the **measured** profile store. We
   therefore (a) evaluate `T_i` only at measured widths — no interpolation/extrapolation, so an
   allocation is always backed by a real measurement; and (b) get the knee and the work-conservation
   break *observed*, not assumed. Analytic models survive only as the fallback prior for a step with
   too few samples.
2. **Single machine, inner-jobs, cgroup-boxed — not a distributed multiprocessor.** The "processors"
   a task receives are inner `-j` threads inside one machine's cgroup CPU budget, not nodes of a
   cluster. There is no data-redistribution or communication cost term (the classic CPA/mixed-parallel
   concern); the only cross-step coupling is **shared CPU, RAM, and named semaphores** on the one box.
   This is why we add the runtime core-budget gate (§5.7) rather than assuming a perfect packing.
3. **Two extra constraint classes the textbook omits.** Standard moldable scheduling constrains only
   processors. We additionally constrain **per-step memory** (and, crucially, memory that *grows with
   width* for CPU-bound steps) and **named scarce-resource semaphores**. Both can *block a widening*
   or *bound achievable concurrency* — first-class inputs to the allocator, not afterthoughts.
4. **A work-conservation (efficiency) stop, not just a wall-time stop.** Textbook CPA stops on the
   pure `T_CP <= T_A` balance. We *additionally* refuse to widen past each step's measured
   work-conservation knee (`cpu_s` growth `> 1.5x` between widths = stop), because on a shared machine
   burning extra core-seconds for a marginal wall gain steals cores from *other* concurrent steps and
   degrades the whole DAG. This efficiency guard is enforced by construction (knee-truncated admissible
   widths, §5.2) and is not part of classical CPA.
5. **Determinism as a hard requirement.** The classical algorithms are described over reals with
   arbitrary tie-breaking. We require **bit-for-bit** identical decisions in two languages, so every
   comparison is at fixed precision with a total tag tie-break (§5.9).

## 7. Testing and rollout

- **Cross-differential.** Add fixtures with multi-width measured curves (a scaling step on the
  critical path, a plateau step off it, a memory-heavy CPU-bound step) and assert the allocator's
  chosen widths, order, and stop-reason are byte-identical py/rs. Extend `cross/differential.py`'s
  `plan --format json` comparison to cover the new `alloc_inner_jobs` field.
- **Unit tests (both builds).** (a) balance point reached on a linear-scaling chain; (b) a plateau
  step is *not* widened; (c) a memory-heavy step is core-free-but-memory-blocked; (d) the core-budget
  gate prevents oversubscription at dispatch; (e) a curveless step stays rigid; (f) idempotence
  (running the allocator twice yields the same widths).
- **Directional benchmark.** On a real DAG (e.g. deepscry's `validate`), compare makespan under
  greedy-lpt vs. per-step-knee vs. the CPA allocator; expect the allocator to win when the DAG has a
  scaling step on a long critical path and contended cores. This is a directional inner-loop
  measurement, not a definitive quiet-machine benchmark.
- **Backward compatibility.** Gate behind a planner/allocator flag; the default stays the current
  behavior until the allocator is validated. `--no-profile-feedback` disables it (no curves => nothing
  to allocate).

## 8. References

Load-bearing citations; web-confirmed where noted.

1. G. M. Amdahl (1967). "Validity of the single processor approach to achieving large scale computing
   capabilities." *AFIPS Spring Joint Computer Conference*, pp. 483–485.
2. R. L. Graham (1969). "Bounds on multiprocessing timing anomalies." *SIAM Journal on Applied
   Mathematics*, 17(2):416–429. (List-scheduling `2 - 1/P` bound; the `max(T_CP, work/P)` intuition.)
3. R. P. Brent (1974). "The parallel evaluation of general arithmetic expressions." *Journal of the
   ACM*, 21(2):201–206. (The `T_p <= T_1/p + T_inf` work/span bound.)
4. T. L. Adam, K. M. Chandy, J. R. Dickson (1974). "A comparison of list schedules for parallel
   processing systems." *Communications of the ACM*, 17(12):685–690. (HLFET — bottom-level ordering.)
5. K. P. Belkhale, P. Banerjee (1990). "An approximate algorithm for the partitionable independent
   task scheduling problem." *International Conference on Parallel Processing (ICPP)*. (Early
   area-balancing allotment.)
6. J. Turek, J. L. Wolf, P. S. Yu (1992). "Approximate algorithms for scheduling parallelizable
   tasks." *4th ACM Symposium on Parallel Algorithms and Architectures (SPAA)*, pp. 323–332.
   (Two-phase allot-then-schedule; 2-approximation for independent malleable tasks.)
7. W. Ludwig, P. Tiwari (1994). "Scheduling malleable and nonmalleable parallel tasks." *5th ACM-SIAM
   Symposium on Discrete Algorithms (SODA)*, pp. 167–176. (Improved allotment via work functions.)
8. A. B. Downey (1997). "A model for speedup of parallel programs." Technical Report UCB/CSD-97-933,
   EECS Department, University of California, Berkeley. (Web-confirmed: report CSD-97-933.)
9. A. Radulescu, A. J. C. van Gemund (2001). "A Low-Cost Approach towards Mixed Task and Data Parallel
   Scheduling." *30th International Conference on Parallel Processing (ICPP)*, pp. 69–76. (**CPA** — the
   critical-path/area-balancing moldable allocator we base this design on.)
10. H. Topcuoglu, S. Hariri, M.-Y. Wu (2002). "Performance-Effective and Low-Complexity Task Scheduling
    for Heterogeneous Computing." *IEEE Transactions on Parallel and Distributed Systems*,
    13(3):260–274. (**HEFT** — cited to explain why its heterogeneous-processor core does not apply to
    our homogeneous single machine.)
11. K. Jansen (2004). "Scheduling malleable parallel tasks: An asymptotic fully polynomial-time
    approximation scheme." *Algorithmica*, 39(1):59–81. (Malleable-task approximation theory.)
12. K. Jansen, H. Zhang (2005/2006). "An approximation algorithm for scheduling malleable tasks under
    general precedence constraints." *ISAAC 2005*; journal version *ACM Transactions on Algorithms*,
    2(3):416–434, 2006. (Web-confirmed title; constant-factor approx for precedence-constrained
    malleable tasks — our theoretical backdrop.)
13. S. Bansal, P. Kumar, K. Singh (2006). "An improved two-step algorithm for task and data parallel
    scheduling in distributed memory machines." *Parallel Computing*, 32(10):759–774. (**MCPA** —
    concurrency-aware CPA; web-confirmed.)

## 9. Where this lives in the code

- Curves: `estimates.load_step_speedups()` → `StepSpeedup` / `SpeedupLevel` (knee = `_build_step_speedup`,
  guards `_SPEEDUP_MIN_MARGINAL_GAIN = 1.15`, `_SPEEDUP_MAX_WORK_GROWTH = 1.5`).
- Order + bottom-levels: `estimates._bottom_levels`, `_critical_path`, `_plan_order`
  (`Planner.CRITICAL_PATH`).
- Memory: `sizing.schedulable_peak_mem_bytes`, `step_mem_cap_for_inner_jobs`, `jobs_for_budget`.
- Core budget: `profile_enrich.container_core_budget()`.
- Named resources + the ready-set loop the core-budget gate extends: `scheduler.Runner`
  (`_res_free` / `_acquire` / `_release`).
- Baking widths in for phase 2: mirror `estimates.apply_plan_to_config`.

The allocator is a new pure function (call it `allocate_widths(cfg, speedups, est, P, mem_budget)
-> Mapping[tag, int]`) added alongside `build_plan`, with a matching Rust implementation and a
cross-differential fixture — the same py/rs + differential discipline every other feature in this
repo follows.
