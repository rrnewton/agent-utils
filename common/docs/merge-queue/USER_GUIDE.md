# merge-queue user guide

merge-queue is a local merge queue for one base branch of one Git repository. You submit
changes as ready to land. The queue orders them, splits them into batches, validates each batch's
exact combined tree, and lands only trees that passed. It runs where your clone is. There is no
hosted service, no webhook and no inbound connection.

This release ships the **planning** half: `init`, `submit`, `withdraw`, `list` and `plan`. The
`run` command, which validates and lands batches, comes next. Its design is described here so
the configuration fields and the validation model already make sense; such passages are marked
*(run)*.

**Requirements.** `git` 2.40 or newer, for `merge-tree --write-tree --merge-base`. The repository
can be a working clone or a bare repository.

## Concepts

- **Change.** One pull request or branch, identified by a positive number (`pr`). It also has a
  head ref, such as a branch name or `refs/pull/N/head`. You submit an exact head commit, and the
  queue never lands any other head for that change.
- **Base.** The branch the queue lands on, for example `main`. Each plan is bound to one exact base
  commit.
- **Batch.** Changes that are validated together and land together, in order. Batches land in
  sequence. *(run)* With speculative depth *d*, up to *d* batches validate at the same time, each
  one on top of the batches before it.
- **Held change.** A queued change that cannot land this round. It always carries a stable kind,
  a reason, and the action that unblocks it.

## Validation is a set of tests

Validation is never treated as one function that returns true or false. It is a large set of
individual tests, and the queue tracks exactly which of them ran.

- **Test IDs.** A test is a runner *step*, optionally narrowed to one *case*. It is written `step`
  or `step::case`, for example `unit.cli` or `unit.cli::parses_flags`. A whole-step ID covers
  every case inside that step. Any runner that can name its steps (and optionally its cases) can
  plug in.
- **Selections.** A selection is either `all` (the complete suite) or an explicit set of test IDs.
  `all` is just one selection among others, not a special mode.
- **Claims.** Every validation, whether the submitter's or the queue's, records its selection, the
  name of the selection policy that chose it (`all`, `path-routing`, `manual`, ...), and its
  scope. The scope is `complete` when the selection is `all`, and `focused` otherwise.
- **Widening obligations.** *(run)* When a batch lands on a focused validation, the queue records
  an obligation to run the complete suite on that commit or a later one. Obligations live in
  `obligations/`, the queue reports them, and a later complete run discharges them.
- **Failed-test bisection.** *(run)* When a batch fails, the queue splits it in halves (as bors-ng
  does) to find the culprit, and reruns **only the failed tests**, plus whatever the runner says
  those tests depend on. It never reruns the whole suite to look at a subset. The culprit is
  ejected with a report, and the queue continues with the rest. There is no automatic repair.
- **Silence is not success.** A test that was selected but never reported counts as an error. If
  a runner reports only an exit status, every selected test inherits that status, and the result
  is marked `unattributed`.

## Global options

| Option | Default | Meaning |
|---|---|---|
| `--repo DIR` | the current directory | The repository the queue operates on. A working tree resolves to its top level. A bare repository resolves to its Git directory. |
| `--state DIR` | `$MERGE_QUEUE_STATE`, else `merge-queue/` inside the Git common directory of `--repo` | The state directory. The default lives in the common directory, so every worktree of a clone shares one queue. |

Exit status: `0` on success, `1` on an error (printed as `merge-queue: ...` on stderr), `2` on a
usage error.

## Commands

### `merge-queue init`

This command creates the state directory and writes `config.json`.

| Option | Default | Meaning |
|---|---|---|
| `--remote NAME` | `origin` | The remote that holds the base branch and the change heads. Pass `--remote ''` for a purely local repository, where heads and base are local branches. |
| `--base BRANCH` | `main` | The branch the queue lands on. |
| `--combine MODE` | `rebase` | `rebase` replays each change's non-merge commits in order, giving linear history like a rebase merge. Merge commits inside a change are dropped, as `git rebase` drops them. `merge` creates one merge commit per change. |
| `--max-batch N` | `4` | The maximum number of changes per batch (at least 1). |
| `--depth N` | `2` | *(run)* How many batches validate at the same time, each on top of the one before (at least 1). |
| `--net-wrapper WORD` | none | A command prefix for network Git operations, for example `--net-wrapper with-proxy`. Repeat it for a prefix of several words. |
| `--force` | off | Rewrite an existing `config.json`. Items and history are kept. |

### `merge-queue submit PR --head REF`

This command submits a change as ready to land, or resubmits it. It takes the writer lock and
fetches the base and this head from the remote into private refs (`refs/merge-queue/...`). It
then records the exact head commit. Resubmitting a change replaces its record, keeps its history,
and sets its status back to `queued`.

| Argument | Default | Meaning |
|---|---|---|
| `PR` | required | The change number (pull-request number). It must be positive. |
| `--head REF` | required | The head branch on the remote, or a full ref such as `refs/pull/42/head`. |
| `--sha SHA` | empty | The exact head commit (or a prefix of it) that you mean. Submission is refused if the branch points elsewhere. |
| `--priority N` | `100` | Lower values land first. |
| `--after PR` | none | A change that must land first. Repeatable. |
| `--solo` | off | Validate and land this change in a batch of its own. Use it for risky changes. |
| `--by NAME` | empty | Who is submitting (an agent or a person). |
| `--title TEXT` | empty | A one-line description. |
| `--tested-with SELECTION` | none | The tests you ran on this head: `all`, a comma- or space-separated list of test IDs, or `@FILE` with one ID per line (`#` starts a comment). This becomes the change's validation claim. Without it, the change has no claim. |
| `--policy NAME` | `manual` | The name of the policy that chose `--tested-with`, for example `path-routing`. |

The output names the head and, when there is a claim, says whether it was focused or complete:
`queued #42 at 9b1c0e77aa (priority 50); validated with 2 tests (focused)`.

### `merge-queue withdraw PR`

This command marks a queued change `withdrawn`. `--reason TEXT` (default `withdrawn by submitter`)
is recorded in the change's history. Submitting the change again puts it back in the queue.

### `merge-queue list`

This command prints the queued changes as a table with the columns `PR`, `STATUS`, `PRIO`, `HEAD`,
`TESTED` and `TITLE`. `TESTED` shows the scope of the submitter's claim (`complete`, `focused`,
or `-` when there is none), so focused submissions are visible at a glance.

| Option | Default | Meaning |
|---|---|---|
| `--all` | off | Include landed, ejected and withdrawn changes. |
| `--json` | off | Print the full item records as a JSON array. |

### `merge-queue plan`

This command fetches the base and every queued head in one pass, probes them locally, and prints
the plan. It changes only the queue's private refs, never the queue state.

| Option | Default | Meaning |
|---|---|---|
| `--max-batch N` | the configured `max_batch` | Override the batch size for this plan. |
| `--json` | off | Print the plan as JSON, using schema `merge-queue-plan/v1` (below). |

The human form lists the batches with each change's priority and file count, then the held
changes, then the conflict clusters (connected groups of conflicting changes with two or more
members).

### `merge-queue quickstart`, `merge-queue userguide`

These commands print the getting-started guide and this guide.

## How a plan is made

Every probe is local, using `git merge-tree`. This is an exact textual merge, not a guess based
on which files two changes share.

1. **Identity.** Each queued change's head ref is resolved again. A head that no longer resolves
   is held as `head-missing`. A head that differs from the submitted commit is held as
   `head-moved`.
2. **Base conflicts.** A change whose head does not merge cleanly with the base is held as
   `conflicts-with-base`.
3. **Dependencies.** A change whose head contains another queued head (a stacked change) must land
   after it. Explicit `--after` dependencies count too. A dependency that has landed is satisfied.
   A dependency that is neither queued nor landed holds the dependent as `waiting-on-dependency`.
   A dependent of a held change is held as `waiting-on-dependency` as well.
4. **Pairwise conflicts.** Every pair of queued heads is merged with `git merge-tree`. Pairs that
   touch no common file, and stacked pairs, are skipped because they cannot conflict.
5. **A conflict-free subset.** The queue does no repair. Once one of two conflicting changes
   lands, the other conflicts with the base, so both cannot land in the same round. The queue
   walks the changes in dependency order and picks the best-ranked ready change each time. The
   rank is: lower priority value first; then fewer conflicts with other eligible changes; then
   earlier submission; then lower number. A change that conflicts with one already chosen is held
   as `conflicts-with-queued`. Preferring fewer conflicts is a greedy maximum-independent-set
   heuristic. When one change conflicts with many independent ones, it lands more changes per
   round than first-come-first-served.
6. **Batches.** The chosen order is cut into batches of at most `max_batch` changes. A `--solo`
   change closes the open batch and gets a batch of its own. Each batch has a `note` saying why
   it was cut: `max-batch`, `solo`, or empty for the last one.

| Held kind | Unblocking action |
|---|---|
| `head-missing` | Resubmit with a head that exists, or withdraw. |
| `head-moved` | Resubmit, so the queue records (and you vouch for) the new head. |
| `conflicts-with-base` | Rebase onto the base and resubmit. |
| `conflicts-with-queued` | Rebase after the named changes land, then resubmit. |
| `waiting-on-dependency` | Submit or unblock the named dependency, or drop `--after`. |

## Plan schema `merge-queue-plan/v1`

```json
{
  "schema": "merge-queue-plan/v1",
  "created_at": "2026-11-01T12:00:00Z",
  "base": "main",
  "base_sha": "<40 hex>",
  "items": [{"pr": 42, "head": "<sha>", "priority": 50, "submitted_at": "...",
             "files": 3, "conflict_degree": 1}],
  "batches": [{"index": 0, "prs": [42, 43], "note": "max-batch"}],
  "held": [{"pr": 41, "kind": "conflicts-with-queued", "reason": "...",
            "with": [42], "paths": ["src/cli.rs"]}],
  "conflicts": [{"a": 41, "b": 42, "paths": ["src/cli.rs"]}],
  "dependencies": [[42, 43]]
}
```

- `items` lists every queued change that was considered. `head` is empty when the head ref did not
  resolve. `conflict_degree` counts conflicts with eligible changes.
- `batches` are in landing order. `index` starts at zero. `prs` are in combine order. `note` is
  left out when empty.
- In `held`, `with` and `paths` are left out when empty.
- `dependencies` holds `[before, after]` pairs that the plan respected.

## State directory

```text
<state>/
  config.json            fixed: queue configuration
  lock                   fixed: writer lock (flock)
  items/<pr>.json        one record per submitted change
  runs/<run-id>/         (run) one directory per run: plan, events, validations, report
  obligations/<id>.json  (run) one record per focused landing still owed a complete run
  scratch/<name>/        (run) candidate worktrees, removed after use
```

Collections that grow live in their own subdirectories, never beside the fixed files. Records are
written atomically, to a temporary file that is then renamed. `submit` and `withdraw` take the
writer lock and fail fast if another writer holds it.

An **item record** (`items/<pr>.json`) has these fields:

- `pr`, `head_ref`, `head`, `priority`, `submitted_at`, `submitted_by`, `title`, `after`, `solo`.
- `claim`: `{selection, policy, scope, commit}`. `selection` is `{"kind":"all"}` or
  `{"kind":"tests","tests":["step","step::case"]}`.
- `status`: `queued`, `landed`, `ejected` or `withdrawn`.
- `landed_as`: the base commit that contains the change, once it has landed.
- `history`: a list of `{at, kind, detail}` events.

## Configuration (`config.json`)

`init` writes this file. You can edit it by hand. Unknown fields are rejected.

| Field | Default | Meaning |
|---|---|---|
| `schema` | `merge-queue-config/v1` | The schema identifier. Other values are refused. |
| `repo` | from `--repo` | The repository directory. |
| `remote` | `origin` from `init` | The remote. Empty means local-only. |
| `base` | `main` | The base branch. |
| `net_wrapper` | `[]` | A command prefix for network Git. |
| `combine` | `rebase` | `rebase` or `merge`. |
| `max_batch` | `4` | The maximum number of changes per batch. |
| `depth` | `2` | *(run)* The speculative depth and validation width. |
| `validator` | empty | *(run)* How validation runs; see below. |
| `land` | empty | *(run)* How a validated candidate reaches the base; see below. |

*(run)* **`validator`** fields:

- `kind`: `command` (a shell template) or `dagrun` (a DAG file, with step-level results).
- `list`: prints the complete inventory.
- `select`: prints the policy's selection for a candidate, given `$MQ_BASE` and `$MQ_HEAD`. Empty
  means the policy is `all`.
- `id_prefix`: only output lines with this prefix are test IDs.
- `policy`: the name recorded for the selection policy.
- `run`: runs a selection.
- `dagrun`, `dag`: the executable and the DAG file, for the `dagrun` kind.
- `admission`: a command prefix that every validation runs under, for example a host admission
  wrapper that limits concurrent validations.
- `timeout_secs`: `0` means no limit.

*(run)* **`land`** fields:

- `driver`: `push` fast-forwards the remote base to the exact validated candidate. `command` runs
  a shell template once per change, in order, and the queue checks the resulting tree against the
  validated one. `none` lands nothing.
- `command`: the shell template for the `command` driver.

## Keeping the base green *(run)*

The design follows the bors rule. The base moves only to a commit whose exact tree passed
validation.

- Candidates are built with Git plumbing, without touching a working tree. A candidate is the base
  plus the batch's changes, in order.
- If the base moves under a batch, the batch is rebuilt and validated again.
- A red batch is bisected on its failed tests, and the culprit is ejected with a report. Under
  speculation, the candidates stacked behind a red batch are discarded and rebuilt.
- After a focused landing, a widening obligation remains until the complete suite has passed on
  that commit or a later one.
