# merge-queue quickstart

**The problem.** Several changes are ready to land on one branch. Landing them one at a time
means one rebase and one validation per change, and the next change has to wait for each. Landing
them all at once breaks the branch when two of them conflict, or when they pass separately but
fail together.

**What merge-queue does.** You submit changes as ready to land. The queue reads the exact
conflict graph from Git, picks a set of changes that can land together this round, orders them
by priority and age, and splits them into batches. Every change it cannot land gets a stated
reason and the action that unblocks it. `run` then validates each batch's exact combined tree,
several at once, and lands it only when it is green.

Validation is a **set of tests**, not one pass/fail answer. A submission records which tests you
ran on it. A focused run leaves a tracked obligation to run the complete suite. When a batch
fails, the search for the culprit reruns only the failed tests, and the culprit is ejected with a
report while the rest lands.

Everything runs locally against a Git clone. There is no service, no webhook and no inbound
connection. The only requirement is `git` 2.40 or newer.

## Five minutes

```bash
cd my-clone                                   # any clone; bare is fine
merge-queue init --remote origin --base main  # state goes in .git/merge-queue/

# Mark changes ready. --tested-with records which tests you ran on that head.
merge-queue submit 41 --head feature/parser --tested-with all
merge-queue submit 42 --head feature/cli --tested-with 'unit.cli,lint' --priority 50
merge-queue submit 43 --head feature/cli-docs --after 42
merge-queue submit 44 --head risky/migration --solo

merge-queue list          # what is queued, and which submissions were only focused
merge-queue plan          # the batches, the held changes, and the conflict clusters
merge-queue plan --json   # the same plan for scripts (schema merge-queue-plan/v1)
```

A plan looks like this:

```text
Plan for main @ 3f2a9c01de: 4 queued, 3 landable in 2 batch(es), 1 held
  batch 1: #42 (p50, 3 files), #43 (p100, 1 files)
  batch 2: #44 (p100, 7 files)  [solo]
Held:
  #41 conflicts-with-queued: conflicts with #42 on src/cli.rs; rebase after it lands and resubmit
Conflict clusters:
  #41, #42
```

## Validate and land

Tell the queue how to run a selection of tests and how to land, then run it:

```bash
merge-queue init --force --remote origin --base main \
  --validator command --validator-run 'sh run-selected.sh' \
  --select 'git diff --name-only "$MQ_BASE" HEAD -- tests' --policy path-routing \
  --land push

merge-queue run --dry-run   # build candidates and selections; validate and land nothing
merge-queue run --shadow    # validate for real, land on a simulated base, change no status
merge-queue run             # validate, land green batches, eject culprits
merge-queue serve --shadow  # daemon soak: same shadow landing, forever; real base never moves
merge-queue obligations     # focused landings still owed a complete run
merge-queue widen           # run the complete suite once and discharge them
```

**Shadow / soak kit.** Rehearse beside the live process without moving the real base. Point the
queue at the hosting service once, then keep it in sync with only the labelled ready set, and run
the daemon in shadow mode:

```bash
merge-queue init --remote origin --base main --github-repo OWNER/NAME --land push
merge-queue sync --label ready   # only open PRs with the `ready` label are the ready set
merge-queue serve --shadow       # plans, validates and would land; real base never moves
```

`sync --label ready` syncs only the labelled ready set: in one batched API call it lists open,
non-draft pull requests carrying `ready`, submits new ones and heads that moved, and withdraws
only what this same label source added when the label is removed. Direct `submit` entries are
never withdrawn by a label sync, drafts are never ready, and with `require_approval_at_head` a
labelled pull request also needs an approving review at its current head. Use `--all-open` only
for measurements that deliberately want every open pull request; a shadow soak that should mirror
what people marked ready uses `--label`.

The runner reads the selected test IDs from `$MQ_TESTS` and writes one JSON line per test to
`$MQ_RESULTS` (`{"id":"t1","outcome":"pass"}`); the user guide has a complete ten-line runner.
With `--depth 2` (the default), two batches validate at once, the second on top of the first;
`init --max-depth 4` lets that window grow on success and halve on a red batch. Each run leaves
`report.md` under `.git/merge-queue/runs/<run-id>/`.

To see what batching and speculation buy before trying it on a real repository:

```bash
merge-queue bench --prs 16 --test-secs 1   # serial lander vs the queue on a synthetic repository
```

## What to know next

- A plan is bound to the exact base commit and to the exact head of each change. If a branch moves
  after you submit it, the change is held as `head-moved` until you submit it again.
- Lower `--priority` lands first (default 100). Among changes of equal priority, the one with
  fewer conflicts is preferred, then the one submitted first.
- A change that fixes CI goes to the front: give it the `mq-priority` label (or submit it with
  `--prioritized`). It is validated first, in a batch of its own, and still lands only on its own
  green.
- `merge-queue withdraw 41` takes a change out of the queue. Submitting it again puts it back with
  its history.
- `merge-queue userguide` documents every command, the state layout, the configuration, the
  validator and land-driver contracts, the plan schema, and how the base stays green.
