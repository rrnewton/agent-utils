# merge-queue quickstart

**The problem.** Several changes are ready to land on one branch. Landing them one at a time
means one rebase and one validation per change, and the next change has to wait for each. Landing
them all at once breaks the branch when two of them conflict, or when they pass separately but
fail together.

**What merge-queue does.** You submit changes as ready to land. The queue reads the exact
conflict graph from Git, picks a set of changes that can land together this round, orders them
by priority and age, and splits them into batches. Every change it cannot land gets a stated
reason and the action that unblocks it. Later commands validate each batch's exact combined tree
and land it only when it is green.

Validation is a **set of tests**, not one pass/fail answer. A submission records which tests you
ran on it. A focused run leaves a tracked obligation to run the complete suite. When a batch
fails, the search for the culprit reruns only the failed tests.

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

## What to know next

- A plan is bound to the exact base commit and to the exact head of each change. If a branch moves
  after you submit it, the change is held as `head-moved` until you submit it again.
- Lower `--priority` lands first (default 100). Among changes of equal priority, the one with
  fewer conflicts is preferred, then the one submitted first.
- `merge-queue withdraw 41` takes a change out of the queue. Submitting it again puts it back with
  its history.
- `merge-queue userguide` documents every command, the state layout, the configuration and the
  plan schema.
