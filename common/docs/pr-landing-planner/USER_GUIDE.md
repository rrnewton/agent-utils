# pr-landing-planner — user guide

`pr-landing-planner` is a **conflict-graph + CI-aware, advisory pull-request landing planner**. Given
the open pull requests targeting a base branch, it computes — in one shot — which PRs truly conflict,
which red CI results are real versus benign, exact-head validation evidence, gate-policy disposition,
assigned agents, mechanism overlaps, freshness, holds, and an ordered action per PR. It is
**advisory only**: it recommends actions and never mutates a pull request. A landing skill or
coordinator executes the mutations.

It generalizes three predecessor scripts into one host-pluggable tool: DeepScry's `pr_interference.py`
(the conflict/ordering-graph → batch mechanic), hermit's `pr_conflict_graph.py` (real `git merge-tree`
conflicts, stacks, held reasoning, the content-identity guard), and hermit's `pr_status.py` (per-PR
CI/label health). The decisive new capability is the **fusion** none of them did: combining the
conflict graph with *live* CI health, freshness, and priority into one actionable plan.

Plain-language note: "landing" means merging a PR into the shared base branch (e.g. `integration`). A
"conflict graph" is a picture of which open PRs collide if merged together. "CI" is the automated
test/build system. "The gate" is the single required check the host waits on before merging.

---

## Install

```sh
pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"
```

Or, from a checkout, `./setup py` and use `./bin/pr-landing-planner`. It needs `gh` and `git` on
`PATH` for live runs (not for `--fixture` runs).

---

## Quick start

Try it with the bundled fixture — no repo, no network:

```sh
pr-landing-planner quickstart --emit-demo > demo.yaml
pr-landing-planner plan --fixture demo.yaml
```

Run against a real repository:

```sh
pr-landing-planner plan \
  --repo OWNER/NAME --base main --git-dir /path/to/clone \
  --net-wrapper with-proxy --gh-cmd ./scripts/gh_human \
  --flaky-signatures flaky-signatures.yaml \
  --landing-context landing-context.json
```

`gh pr list` supplies the PRs (with the CI rollup + labels); `git merge-tree` finds the real
conflicts against a local clone.

---

## Subcommands

| Command | What it prints |
|---------|----------------|
| `plan` (default) | the full landing plan: parallel-safe groups, land-now set, order, per-PR actions, and diagnostics |
| `graph` | just the conflict/ordering graph (nodes, real conflicts, file-overlap risks, stacks, held) |
| `status` | just per-PR CI/label health, with an open-PR-count warning |
| `quickstart` | a self-contained getting-started tour (add `--emit-demo` to print only the demo fixture) |
| `--userguide` | this full guide (the complete reference) |
| `--version`, `-h/--help` | version / colored help |

---

## The core algorithm

1. **Collect** the open PRs from the host and select those targeting `--base` plus their transitive
   stacks (and any `--prs` restriction).
2. **Fetch** the base ref and each PR head, then run the **content-identity guard**: if a fetched head
   commit differs from the API's reported head, abort with "rerun" so the plan is never built from a
   half-updated snapshot.
3. **Build the conflict graph.** `--conflict-detector merge-tree` (default) runs `git merge-tree` on
   each pair to detect *real* merge conflicts without touching a worktree; `--conflict-detector
   file-overlap` is a fast, conservative fallback that treats any shared-file pair as a conflict (it
   over-serializes PRs that share a file but auto-merge cleanly). File-overlap edges are always
   computed separately as a weaker "semantic-review risk" signal.
4. **Build ordering edges** from explicit base-branch stacking and from git ancestry (so that when
   you rebase PR B onto PR A's tip, the next run detects the new ancestry and the pair becomes a
   satisfied ordering constraint). Ordering edges are transitively reduced; stacks are extracted.
5. **Classify each PR's CI** (see below), apply exact-head caller context, and compute **freshness**.
6. **Compute held reasons**: `draft`, `local-base-conflict` (a real merge-tree conflict with the
   base), `github-base-conflicting`, and `depends-on-held:#N` (propagated transitively).
7. **Partition** the non-held PRs into **parallel-safe groups**: a greedy independent-set layering
   over the conflict graph that respects ordering edges. PRs in a group are safe to land / review in
   parallel.
8. **Surface mechanism overlaps** from shared `mechanism:<slug>` labels. They request coordinator
   review but do not prove conflicting intent.
9. **Assign each PR an action** via the fusion table, ordered priority → diff size → age → PR number.

### Landing context

GitHub does not know a caller's local validation ledger, task assignment, or policy classification.
Pass those facts in a JSON or YAML file. Clean validation evidence is accepted only when guarded by
the exact current head SHA:

```json
{"prs":[{"pr":123,"head_sha":"<40-hex>","validation_evidence":"clean-validate-record","policy_class":"ci-hygiene","assigned_agent":"agent-name"}]}
```

`validation_evidence` is `clean-validate-record`, `locally-validated`, `authoritative-ci`, or `none`.
`policy_class` is `ci-hygiene`, `gate-policy`, or `unclassified`. Labels can also provide
`locally-validated`, `agent:<name>`, `landing-policy:<class>`, and `mechanism:<slug>`. The
`locally-validated` value records that the cache label was observed; it never authorizes landing.
Only authoritative CI or a caller-supplied `clean-validate-record` bound to the exact head does.

---

## The five red classifications (the headline value)

A naive lander treats every red check as a failure and is wildly wrong — a CI-health analysis of one
24-hour window found 53% of failures were benign gate noise. This tool instead classifies **why** a
PR is red, into five modes grounded in real incidents:

| Class | Meaning | Recommended action |
|-------|---------|--------------------|
| `real` | a genuine regression | `hold-fix` |
| `flaky` | a red whose check name/message matches a caller signature | `refire-ci` |
| `stale-required-check` | the underlying CI is green on the head, but the required gate froze on a stale result (ds-4171) | `refire-stale-gate` |
| `evaluate-once-race` | the gate fired once while full CI was still queued and exited "still queued" (ds-xdc7m9 / ds-96k1wa) | `wait` (benign; treated as pending) |
| `runner-outage` | the gate job never actually ran (blank runner / `BlobNotFound` / near-zero duration), usually across many branches (ds-69ih3r) | `escalate-runner-outage` |

Precedence for a red PR: outage → evaluate-once race → stale required check → flaky → real. A systemic
outage is declared when at least `--outage-min-prs` PRs (default 2) show the gate-never-ran signature.

Nothing project-specific is baked in: the gate-check name (`--gate-check`, default `merge-gate`), the
flaky signatures (`--flaky-signatures FILE`), and the race/outage markers are all caller config.

### Flaky signatures file

A JSON or YAML file (a top-level list, or `{signatures: [...]}`). Each entry has optional `name_regex`
and/or `text_regex` (at least one required) and an optional `note`. A red check matches when its name
matches `name_regex` (if set) AND its message matches `text_regex` (if set):

```yaml
signatures:
  - name_regex: "wasm-core"
    note: "known-flaky WASM browser test"
  - text_regex: "font.*abort|equivalence seed=315"
    note: "known flaky messages"
```

Keep this file curated and owned — a stale signature silently masks a real regression.

---

## Per-PR actions

| Action | When |
|--------|------|
| `land-now` | authoritative CI is green, or exact-head local evidence is present; fresh enough to land |
| `rebase-then-land` | green but more than `--freshness-max-behind` commits behind base, OR held on a base conflict |
| `refire-stale-gate` | CI green on head; the required gate is stale |
| `escalate-runner-outage` | the gate job never ran |
| `escalate-gate-policy` | the PR changes landing/gate policy; validation is not policy approval |
| `refire-ci` | a flaky red |
| `hold-fix` | a real red |
| `wait` | pending, no checks, an evaluate-once race, a draft, or depends-on-held |

---

## Priority (ordering)

`--priority-source` selects how the land-now set is ranked (lower priority number = more urgent; ties
break by diff size, then age, then PR number):

- `none` (default) — every PR is equal; ordering falls back to size then age.
- `labels` — parse an integer from a label matching `--priority-label-pattern` (default matches `p0`,
  `p1`, `priority-2`, `priority:3`, …).
- `beads` — run `--priority-cmd` per PR (with `{pr}` substituted); its stdout's first token is the
  integer priority. Failures fall back to the default priority (visibly, never silently).

---

## Output formats (`--format`)

- `human` (default) — a readable landing summary.
- `json` — the full machine schema, deterministic (2-space indent, sorted keys). Schema top level:
  `repository`, `base`, `nodes[]`, `conflict_edges[]`, `file_overlap_edges[]`,
  `mechanism_overlap_edges[]`, `ordering_edges[]`,
  `stacks[]`, `held_prs[]`, `plan{parallel_safe_groups, land_now, order, batch, per_pr_actions[]}`,
  `diagnostics{stale_gates, flaky_reds, real_reds, evaluate_once_race, outage_prs, outage_suspected}`.
  Each node includes `assigned_agent`, `validation_evidence`, and `policy_class`.
- `actions` — tick-hub-style line output: a block of bare `key=value` summary counts (so a tick-hub
  reminder's `capture: true` gate can lift `land_now` / `stale_gates` / `outage` into its emitted
  line), then loud diagnostic `ERROR:` / `NOTE:` lines, then one `ACTION:` / `ERROR:` / `NOTE:` line
  per PR in the recommended order. A coordinator parses the per-PR lines by their leading token.

`graph` and `status` accept `--format {human,json}`.

---

## Batch mode (`--batch`, off by default)

`--batch` additionally proposes **one green-only, conflict-free batch** to arm behind a single gate
(bors-style amortization). It is off by default because CI is the scarce resource; enable it when the
runner pool is idle and the queue is deep. On a batch failure, bisect the batch to find the culprit
and re-batch the innocents (documented fallback; the planner does not bisect for you).

---

## tick-hub integration (Option B — zero tick-hub change)

`pr-landing-planner` is the data source for a PR-landing reminder on a single ops tick. Wire a
[tick-hub](../tick-hub/USER_GUIDE.md) reminder whose gate runs the planner in `actions` format and
captures its summary counts, then emits one `ACTION` dispatching your landing skill:

```yaml
- name: pr_landing
  cadence_secs: 1800                 # ~ ops tick interval
  requires_flags: [ops_in_charge]
  gate:
    cmd: "pr-landing-planner plan --format actions --net-wrapper with-proxy --gh-cmd ./scripts/gh_human --git-dir /path/to/clone"
    when: nonempty
    capture: true                    # lifts land_now / stale_gates / outage / ... into fields
  emit:
    kind: action
    skill: landing
    title: "landing: {land_now} ready, {stale_gates} stale-gate, outage={outage}"
```

The tick's `capture: true` parses the planner's bare `key=value` summary lines; the dispatched
`landing` skill then re-runs the planner in full (`--format human` or `json`) for the detailed plan.
The planner's own `ERROR:` (systemic outage) and `NOTE:` (evaluate-once race) lines surface loudly so
nothing is silently dropped. This requires no change to tick-hub. (An optional tick-hub "pass-through
emit" mode that forwards the planner's per-PR lines verbatim is a possible future enhancement, not
needed to ship.)

---

## Host pluggability

The only side-effecting boundary is the `VcsHost` protocol (list PRs; and per-PR git operations:
fetch-ref, merge-tree, is-ancestor, changed-files, commits-behind). The shipped implementation is
`GitHubHost` (`gh` + `git`, honoring `--net-wrapper` and `--gh-cmd`); `FakeHost` backs the
deterministic tests and the `--fixture` demo. A GitLab / Gerrit host would implement the same
protocol; the pure core (graph / classify / plan / emit) never changes.

### Fixture format (for `--fixture`)

A JSON or YAML document: `repo`, `base`, a `prs` list (each with `number`, `head_ref`, `base_ref`,
optional `head_sha` / `api_head_sha` / `fetched_head_sha`, `is_draft`, `mergeable`, `labels`,
`commits_behind`, `changed_files`, `base_conflict_paths`, and a `checks` list of
`{name, status, conclusion, text, duration_secs}`), an optional `conflicts` list of `{a, b, paths}`
(real merge-tree conflicts), and an optional `ancestry` list of `{before, after}`. Setting a PR's
`fetched_head_sha` different from its `api_head_sha` simulates a mid-collection change to exercise the
content-identity guard.

---

## Exit codes

- `0` — the command ran (the *content* of the plan carries the reds/holds).
- `2` — bad usage, or a host / fixture error (including the content-identity-guard "rerun" abort).

---

## Relationship to the predecessors

This tool subsumes `pr_interference.py`, `pr_conflict_graph.py`, and `pr_status.py`. Those scripts are
intentionally NOT retired yet — retiring them (and pointing their callers, e.g. DeepScry's `landing`
skill, at this tool) is a follow-up once a caller has adopted `pr-landing-planner`.

A Rust port with a `cross/` byte-identical differential (over the pure graph/classify/plan/emit core)
is a documented follow-up, mirroring how the other agent-utils tools began Python-first.
