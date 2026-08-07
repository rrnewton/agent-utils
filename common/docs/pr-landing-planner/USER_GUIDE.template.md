# pr-landing-planner user guide

`pr-landing-planner` fuses pull-request metadata, exact fetched heads, merge
conflicts, CI evidence, holds, freshness, and semantic overlaps into an
advisory landing plan. Its outputs are deterministic for a fixed snapshot and
it performs no repository-host mutations.

{{DISTRIBUTION}}

## Start with an offline fixture

```sh
pr-landing-planner quickstart --emit-demo > demo.yaml
pr-landing-planner plan --fixture demo.yaml
pr-landing-planner graph --fixture demo.yaml
pr-landing-planner status --fixture demo.yaml
```

The emitted demo is a complete JSON/YAML fixture with pull requests, checks,
heads, changed files, conflicts, and ancestry. Fixture mode is deterministic,
network-free, and does not archive a plan unless `--archive-dir` is explicit.
Duplicate mapping keys, nonpositive or overflowing identifiers, duplicate PR
identities, negative numeric counters, and relations with self/unknown endpoints
are rejected instead of being silently normalized.

Use fixture mode for evaluation, tests, saved snapshots, and integrations that
already collect repository-host data.

## Plan a live repository

```sh
pr-landing-planner plan \
  --repo OWNER/NAME \
  --base main \
  --git-dir /path/to/clone
```

`--repo` is required for live runs. The host adapter reads PR metadata with
`gh`, fetches exact base and PR heads into private local refs, and uses `git
merge-tree` to detect real conflicts. It does not check out, rebase, push,
label, refire, or merge anything.

Common collection flags include:

| Flag | Meaning |
|---|---|
| `--base BRANCH` | Target branch; default `main`. |
| `--prs N,N,...` | Restrict collection to selected PR numbers. |
| `--git-dir PATH` | Local clone used for fetch and merge analysis. |
| `--remote NAME` | Remote to fetch; default `origin`. |
| `--conflict-detector merge-tree` | Exact merge conflict detection; the default. |
| `--conflict-detector file-overlap` | Faster conservative fallback. |
| `--net-wrapper PREFIX` | Optional command prefix for host commands. |
| `--gh-cmd COMMAND` | Alternate `gh` executable or wrapper. |

Content-identity checks fail closed if the fetched head differs from host
metadata or if caller evidence names a different fetched head or base.

## What the graph means

The planner distinguishes several relationships:

- **conflict edges** mean two exact PR heads cannot merge cleanly together;
- **ordering edges** encode an explicit stack/dependency direction;
- **file-overlap edges** are informative changed-path intersections;
- **mechanism-overlap edges** flag changes to the same recognized operational
  mechanism even when their files differ; and
- **unclassified mechanism candidates** stay visible instead of being silently
  forced into the wrong category.

Parallel-safe groups contain PRs that do not conflict under the chosen model.
Clusters retain connected conflict sets so a coordinator can reason about
stacking and avoid needless repeated rebases.

## CI classification

The required gate name is caller-configurable with `--gate-check`. Red or
apparently red states are classified before actions are chosen:

| Class | Interpretation | Typical recommendation |
|---|---|---|
| `real` | A genuine check failure. | `hold-fix` |
| `flaky` | Matches a supplied check/message signature. | `refire-ci` |
| `stale-required-check` | Underlying checks passed but the gate is stale. | `refire-stale-gate` |
| `evaluate-once-race` | The gate evaluated while required work was queued. | `wait` |
| `runner-outage` | The gate job did not actually execute. | `escalate-runner-outage` |

Supply flaky signatures as strict JSON or YAML:

```yaml
- name_regex: '^integration$'
  text_regex: 'temporary connection reset'
```

```sh
pr-landing-planner plan \
  --repo OWNER/NAME \
  --git-dir /path/to/clone \
  --flaky-signatures flaky.yaml
```

`--outage-min-prs` controls how many simultaneous outage signatures establish
a systemic outage.

## Exact head/base landing context

Some facts belong to the caller rather than the repository host: an assigned
agent, exact head/base local validation, exact-head adversarial-review receipts,
and whether a PR changes gate policy.
Provide them with `--landing-context`:

```yaml
prs:
  - pr: 42
    head_sha: 0123456789abcdef
    base_sha: fedcba9876543210
    assigned_agent: release-coordinator
    validation_evidence: clean-validate-record
    review_pass_heads:
      codex: 0123456789abcdef0123456789abcdef01234567
      claude: 0123456789abcdef0123456789abcdef01234567
    policy_class: ci-hygiene
```

Accepted validation evidence is `none`, `authoritative-ci`,
`locally-validated`, or `clean-validate-record`. A clean validation record must
name both the exact fetched head and base SHAs. A head-only record is incomplete
and is rejected with a revalidation instruction. Accepted policy classes are `unclassified`,
`ci-hygiene`, and `gate-policy`; a gate-policy change is escalated rather than
treated like routine hygiene. Unknown PRs, duplicate entries, and head drift
are errors.

`review_pass_heads` maps each required review lane (`codex` and `claude`) to the
exact 40-character lowercase head SHA that reviewer passed. Review labels are
cache hints: when the review protocol is active, each lane needs both its
`passed-review-LANE` label and an exact-head receipt. A missing receipt is
`unbound`; a different head is `stale`. Any rebase, including one with an
identical patch-id, therefore requires a bounded delta re-check and a new
exact-head receipt. The JSON node reports `review_binding` and
`review_pass_heads`; stale or incomplete bindings are held.

## Freshness, holds, priority, and batching

`--freshness-max-behind N` recommends a rebase when an otherwise landable PR is
more than `N` commits behind. Draft state, missing approvals, conflicts,
ordering constraints, CI state, and policy escalation can hold a PR.

Priority defaults to deterministic size and age ordering. `--priority-source
labels` reads a numeric label matching `--priority-label-pattern`; a configured
`--priority-source command` with `--priority-cmd` can supply an integer priority.
The command is required and must print exactly one signed 64-bit ASCII integer;
malformed priority configuration or output stops planning. Ties remain deterministic.

`--batch` additionally proposes one green batch containing only dependency-root
PRs that do not conflict with one another. Ordered children wait for a later
snapshot. The result is still advisory and never arms a queue.

## Output formats and archives

```sh
pr-landing-planner plan --fixture demo.yaml --format human
pr-landing-planner plan --fixture demo.yaml --format json
pr-landing-planner plan --fixture demo.yaml --format actions
```

`human` is for terminals. `json` is the complete stable machine schema.
`actions` emits one line per recommendation for simple automation. Graph,
cluster, and status views support human and JSON formats.

Live plans are archived as canonical JSON in an operating-system state
directory by default. Override it with `--archive-dir DIR` or
`PR_LANDING_PLANNER_ARCHIVE_DIR`; disable it with `--no-archive`. Archive notes
go to standard error so standard output remains pure.

## Command summary

| Command | Purpose |
|---|---|
| `plan` | Build the fused graph and recommended per-PR actions. |
| `graph` | Show only conflict and ordering structure. |
| `clusters` | Group shared conflict sets into stack-landing lanes. |
| `status` | Show per-PR CI and label health. |
| `quickstart` | Print an introduction or emit a demo fixture. |

All commands support `--help`; the top level supports `--version` and
`--userguide`.

## Safety model

Treat every recommendation as a decision aid. The collector's network and Git
operations are read-only with respect to the repository host, but live mode
does update private refs in the supplied clone and may write the local plan
archive. A separate, authorized actor must perform any refire, rebase, label,
landing, or merge.

## License

MIT
