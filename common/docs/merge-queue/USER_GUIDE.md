# merge-queue user guide

merge-queue is a local merge queue for one base branch of one Git repository. You submit
changes as ready to land. The queue orders them, splits them into batches, validates each batch's
exact combined tree, and lands only trees that passed. It runs where your clone is. There is no
hosted service, no webhook and no inbound connection.

Commands: `init`, `submit`, `withdraw`, `list` and `plan` decide what can land; `run` builds,
validates and lands it; `serve` runs that loop forever so nobody has to drive it by hand, and
`status` reports whether the daemon is alive; `widen` and `obligations` track focused landings
until the complete suite has run; `bench` measures the landing rate against a serial lander on
a synthetic repository.

**Requirements.** `git` 2.40 or newer, for `merge-tree --write-tree --merge-base`. The repository
can be a working clone or a bare repository.

## Concepts

- **Change.** One pull request or branch, identified by a positive number (`pr`). It also has a
  head ref, such as a branch name or `refs/pull/N/head`. You submit an exact head commit, and the
  queue never lands any other head for that change.
- **Base.** The branch the queue lands on, for example `main`. Each plan is bound to one exact base
  commit.
- **Batch.** Changes that are validated together and land together, in order. Batches land in
  sequence. A rolling window of batches validates at the same time, each one on top of the batches
  before it; the window starts at `depth` and adapts (see "Keeping the base green").
- **Candidate.** The base plus a batch's changes, combined in order with Git plumbing (no working
  tree is touched). A candidate also keeps the commit after each change (its *prefixes*), which the
  culprit search and the `command` land driver use.
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
- **Widening obligations.** When a batch lands on a focused validation, the queue records
  an obligation to run the complete suite on that commit or a later one. Obligations live in
  `obligations/`, the queue reports them, and a later complete run discharges them.
- **Failed-test culprit search.** When a batch fails, the queue searches the batch's prefixes for
  the first change that makes the failure appear, and every probe runs **only the failed tests**
  (plus whatever the runner says those tests depend on). It never reruns the whole suite to look
  at a subset. A change is blamed only when its failure was seen twice. The culprit is ejected with
  a report, and the queue continues with the rest. There is no automatic repair. See "Keeping the
  base green".
- **Silence is not success.** A test that was selected but never reported has outcome `missing`,
  and it blocks landing. In a culprit search, though, only tests that actually failed or errored
  (or a non-zero exit) count as reproducing the failure. A test that a change adds does not exist
  at earlier commits and cannot fail there, so the search never blames the base for it. If a
  runner reports only an exit status, every selected test inherits that status, and the result is
  marked `unattributed`.

## Global options

| Option | Default | Meaning |
|---|---|---|
| `--repo DIR` | the current directory | The repository the queue operates on. A working tree resolves to its top level. A bare repository resolves to its Git directory. |
| `--state DIR` | `$MERGE_QUEUE_STATE`, else `merge-queue/` inside the Git common directory of `--repo` | The state directory. The default lives in the common directory, so every worktree of a clone shares one queue. |

Exit status: `0` on success, `1` on an error before any work started (printed as
`merge-queue: ...` on stderr), `2` on a usage error, `3` when `run` stopped early (broken base,
landing failure, or an error during the run, which the report records as `stopped: "error: ..."`)
or `widen` found the complete suite red. `status` exits `0` only when the daemon is running
with a fresh heartbeat, and `1` when it is not running, dead, stale or malformed (the output
names the verdict and the remedy). Exit codes 126 and 127 are never success. `status` exits `0` only when the daemon is running
with a fresh heartbeat, and `1` when it is not running, dead, stale or malformed (the output
names the verdict and the remedy). Exit codes 126 and 127 are never success.

## Commands

### `merge-queue init`

This command creates the state directory and writes `config.json`.

| Option | Default | Meaning |
|---|---|---|
| `--remote NAME` | `origin` | The remote that holds the base branch and the change heads. Pass `--remote ''` for a purely local repository, where heads and base are local branches. |
| `--base BRANCH` | `main` | The branch the queue lands on. |
| `--combine MODE` | `rebase` | `rebase` replays each change's non-merge commits in order, giving linear history like a rebase merge. Merge commits inside a change are dropped, as `git rebase` drops them. `merge` creates one merge commit per change. |
| `--max-batch N` | `4` | The maximum number of changes per batch (at least 1). |
| `--depth N` | `2` | The initial window (batches validating at the same time) and the number of concurrent probes during a culprit search (at least 1). |
| `--max-depth N` | `0` | The largest the adaptive window may grow: +1 per landed batch, halved per red batch, never below 1. `0` keeps the window fixed at `--depth`. |
| `--union-path PATTERN` | none | A Git attribute pattern merged with Git's `union` driver, when probing conflicts and when combining changes. Repeatable. Only for line-oriented, append-only files (see "Union merges"). |
| `--regen-path PATTERN` | none | A generated file (exact path, `dir/`, or `*` wildcards) whose conflicts the queue resolves by regenerating it instead of holding the change. Repeatable. See "Generated files". |
| `--regen-command CMD` | empty | Shell command, run at a checkout's root, that rewrites every generated file from its sources. Required with `--regen-path`. |
| `--regen-when WHEN` | `conflict` | `conflict`: regenerate only when a change's conflicts are confined to generated files. `always`: also after every change, so a combination never leaves a generated file stale. |
| `--exact-sha` | off | Exact-commit rule: never carry a validation result to another commit. See "The exact-commit rule". |
| `--trust-base` | off | Treat the base as green in culprit searches and skip probing it. Sound when only this queue lands on the base, always as exactly validated commits; a broken base would then be blamed on the first change of a red batch. |
| `--classify PATTERN=CLASS` | none | Label failed tests whose ID, or whose step alone, matches `PATTERN` (exact or `*` wildcards) with `CLASS` in ejection records, reports and item history. One example is `--classify 'test.*_count=count pin drift'`. Repeatable. |
| `--peer NAME=DIR` | none | A peer queue: another repository's merge-queue state directory. Repeatable. See "Changes across repositories". |
| `--identity-name NAME`, `--identity-email EMAIL` | Git's configuration | Identity for commits the queue creates: committer of replayed commits, author and committer of merge and regeneration commits. |
| `--github-repo OWNER/NAME` | empty | Hosting repository for `sync --label`, `sync --all-open` and `--close-prs`. |
| `--gh WORD` | `gh` | Command (and leading arguments) to run instead of `gh`, for example a pacing wrapper. Repeatable. |
| `--api-per-hour N` | `0` (= 60) | API requests this queue may make per rolling hour. See "Ready sources and the request budget". |
| `--api-budget-file PATH` | state directory | Budget file to share with every other queue on the host, so they all draw on one hourly allowance. |
| `--anonymous-fetch` | off | Fetch without credentials (public repositories). Credential helpers are disabled for fetches, so a fetch never spends a credential use. |
| `--close-prs` | off | After landing, close each landed pull request with a comment naming the landed commit. Uses the budget; a close it cannot make now is retried by `sync`. |
| `--agentctl WORD` | `agentctl` | Command (and leading arguments) the `serve` daemon runs instead of `agentctl` for state-change notifications. Repeatable. |
| `--net-wrapper WORD` | none | A command prefix for network Git operations, for example `--net-wrapper with-proxy`. Repeat it for a prefix of several words. |
| `--validator KIND` | empty | `command` or `dagrun` (see "Validators"). `run` refuses to start without one. |
| `--validator-run CMD` | empty | For `command`: the shell template that runs a selection. |
| `--select CMD` | empty | Shell command printing the selection policy's test IDs for the checkout's HEAD relative to `$MQ_BASE`. Empty: every validation runs the complete suite. |
| `--id-prefix TEXT` | empty | Only lines of `--select` output that start with this prefix are test IDs (the prefix is removed), for example `STEP `. |
| `--policy NAME` | empty | The name recorded for the selection policy (empty records `select-command`, or `all` without `--select`). |
| `--dag FILE` | empty | For `dagrun`: the DAG file, relative to the candidate checkout. |
| `--dagrun PATH` | empty | For `dagrun`: the dagrun executable, relative to the checkout or absolute (empty means `dagrun` on `PATH`). |
| `--admission WORD` | none | A command prefix every validation runs under, such as a host admission wrapper. Repeat it for several words. |
| `--refusal-max-retries N` | `3` | Extra attempts after an admission refusal (exit `75` or `COULD_NOT_RUN`) within one `run`, each after a backoff delay. A refusal is never a pass and never red: the change stays queued. |
| `--refusal-backoff-secs SECS` | `2` | Base backoff between refusal retries, doubled per attempt and capped at 60 seconds. `0` means no delay. The same backoff sets the deferred change's next retry time. |
| `--timeout SECS` | `0` | Seconds before one validation is killed (its whole process group) and its selected tests count as errors. `0` means no limit. |
| `--clean-ignored` | off | Also delete ignored files (build caches) from a reused checkout before each validation. By default they are kept, which makes validations fast but trusts the build to be hermetic with respect to them. |
| `--land DRIVER` | `push` | `push` or `command` (see "Land drivers"). |
| `--land-command CMD` | empty | For `--land command`: the shell template run once per change. |
| `--allow-batches` | off | For `--land command`: allow batches of several changes. Without it every batch is one change, because a per-change driver passes the base through intermediate trees that were never validated on their own. |
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
| `--notify NAME` | empty | agentctl session name the `serve` daemon notifies exactly once per state change (landed, ejected, refusal-deferred, withdrawn). Stored on the item next to `submitted_by`; resubmitting without it keeps the recorded name. Ready files set it per line with `notify=NAME`. |
| `--notify NAME` | empty | agentctl session name the `serve` daemon notifies exactly once per state change (landed, ejected, refusal-deferred, withdrawn). Stored on the item next to `submitted_by`; resubmitting without it keeps the recorded name. Ready files set it per line with `notify=NAME`. |
| `--title TEXT` | empty | A one-line description. |
| `--tested-with SELECTION` | none | The tests you ran on this head: `all`, a comma- or space-separated list of test IDs, or `@FILE` with one ID per line (`#` starts a comment). This becomes the change's validation claim. Without it, the change has no claim. |
| `--policy NAME` | `manual` | The name of the policy that chose `--tested-with`, for example `path-routing`. |
| `--requires SPEC` | none | A commit in a peer repository that must be on that repository's base first: `PEER#PR`, the head of that peer change (which is then marked to land as its own commits), or `PEER@SHA`. Repeatable. See "Changes across repositories". |

The output names the head and, when there is a claim, says whether it was focused or complete:
`queued #42 at 9b1c0e77aa (priority 50); validated with 2 tests (focused)`.

### `merge-queue sync`

Reconcile the queue with a ready source. Exactly one of these is required:

| Option | Default | Meaning |
|---|---|---|
| `--label LABEL` | none | Ready = open pull requests carrying `LABEL`, in one batched API call. |
| `--all-open` | off | Ready = every open, non-draft pull request targeting the base, in one batched API call. |
| `--file FILE` | none | Ready = the changes listed in `FILE`, one per line. Makes no API call. |
| `--limit N` | `300` | Most pull requests to list. Each 100 costs one API request. |
| `--dry-run` | off | Report what would change without submitting or withdrawing anything. The list call is still made. |
| `--json` | off | Print the report as JSON. |

`sync` submits new changes and changes whose head or priority moved, with one fetch for all of
them. It withdraws only changes that this same source added and no longer lists. A change you
submitted directly is never withdrawn by a source. Draft pull requests are never ready.
Priorities come from labels such as `p10` or `priority-10`, the landing planner's convention;
lower lands first, and the default is 100. When `--close-prs` is set, `sync` also retries closes
that were postponed. Exit status is 1 if any submission was refused.

Ready-file lines look like `PR [SHA] [head=REF] [priority=N] [after=N,N] [solo] [by=NAME] [notify=NAME]`, and
`#` starts a comment. Without `head=`, the head is `refs/pull/PR/head`.

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

### `merge-queue run`

This command takes the writer lock, makes a fresh plan, and works through its batches: it builds
candidates, validates them, lands the green ones in order, searches red ones for culprits, and
ejects them. It writes everything to `runs/<run-id>/` and prints the run report.

| Option | Default | Meaning |
|---|---|---|
| `--dry-run` | off | Build the candidates (each on a simulated base made of the ones before) and compute their selections, then stop: no validation, no landing, no change to any item or obligation. The report lists them under `would_validate`. |
| `--shadow` | off | Validate and search for culprits for real, but land on a simulated base kept in memory instead of the real one, and change no item or obligation. A rehearsal beside the existing process. |
| `--depth N` | the configured `depth` | Override the initial window and the probe width. |
| `--max-depth N` | the configured `max_depth` | Override the largest adaptive window. |
| `--max-batch N` | the configured `max_batch` | Override the batch size for this run's plan. |
| `--max-batches N` | `0` | Stop after this many batches. `0` drains the plan. |
| `--widen` | off | After draining, run the complete suite once to discharge open obligations. Skipped in `--shadow` and `--dry-run` runs, because obligations are real state. |
| `--with-peers` | off | First drain each peer queue that holds a change some queued change here requires; the report lists those runs under `peer_runs`. |
| `--json` | off | Print the report as JSON instead of Markdown. |

The exit status is `3` when the run stopped early; the report's `stopped` field says why. An
error in the middle of a run (a Git or validator failure) stops it the same way, and the report is
still written.

### `merge-queue serve`

This command runs the queue forever: deterministic code that is always running, so agents submit
changes and never drive the queue loop by hand. While idle it holds **no** writer lock. It polls
the `items/` directory fingerprint every `--poll-ms` (default `250`) and polls the remote base
with a cheap `git ls-remote` every `--base-poll-secs` (default `30`). A changed fingerprint (a
new `submit`, `sync` or `withdraw`) or a moved base triggers one work cycle: the daemon takes the
writer lock, plans, and runs the same `Runner` path as `run` (the logic is not forked), then
releases the lock and goes back to idle polling. A cycle that finds the lock held, a refused
validation, or a failed base poll is recorded in the heartbeat and retried on the next trigger;
it never kills the daemon. Stop it with SIGINT or SIGTERM (Ctrl-C); a graceful stop removes the
pid file, a crash leaves it behind for `status` to report.

| Option | Default | Meaning |
|---|---|---|
| `--poll-ms N` | `250` | Idle poll interval in milliseconds: `items/` watch and heartbeat cadence (at least 10). |
| `--base-poll-secs N` | `30` | Seconds between `git ls-remote` base polls. `0` polls the base every cycle. |
| `--once` | off | Run at most one work cycle, then exit. For scripts and tests. |
| `--depth N` | the configured `depth` | Override the initial window for work cycles. |
| `--max-batch N` | the configured `max_batch` | Override the batch size for work cycles. |
| `--agentctl WORD` | configured | Command (and leading arguments) to run instead of the configured agentctl for notifications. Repeatable. Supplement: `MERGE_QUEUE_AGENTCTL` (shell-style words) when neither flag nor config names one. |

## Notifications

A change submitted with `--notify NAME` (or a ready-file `notify=NAME`) gets exactly one push message per state change — `landed`, `ejected`, `refusal-deferred`, `withdrawn` — from the `serve` daemon:

```text
agentctl send NAME --deliver-while-working --file <state>/notifications/<report>.md
```

The report names the change, the exact candidate (submitted head) SHA, the verdict, the landed base commit (or the failing tests / refusal detail and retry time), and the run-report path. Delivery state (transition key, attempts, delivered flag, last error) is persisted on the item record as `notify_delivery`, so daemon restarts and extra cycles never resend a delivered transition. A failed send — including a missing agentctl binary (126/127 are failures, never success) — is logged to the daemon's stderr and the item's history, retried on later cycles at most 3 times in total, and never blocks or fails the queue. The command is configured with `init --agentctl WORD...` (stored as `notify.agentctl`), overridden per daemon with `serve --agentctl WORD...`, and supplemented by `MERGE_QUEUE_AGENTCTL`.

### `merge-queue status`

This command reports daemon liveness, the current window and slot occupancy without taking the
writer lock. Liveness comes from `serve.pid` and `serve.heartbeat.json` in the state directory
(the daemon rewrites the heartbeat every cycle): `running` (pid alive, heartbeat fresh), `not
running` (no pid file), `dead` (pid file names a dead process), `stale` (pid alive but the
heartbeat is older than `--stale-after-secs`), or `malformed` (pid file unreadable). Every
non-running verdict names its remedy. The window is the daemon's active batches (or the newest
run's plan when no daemon heartbeat exists); slots count the `scratch/slot-*` checkouts.

| Option | Default | Meaning |
|---|---|---|
| `--json` | off | Print the status report as JSON (`daemon`, `pid`, `heartbeat`, queue counts, `active_batches`, slots, `remedy`). |
| `--stale-after-secs N` | `10` | Heartbeat age in seconds after which a live daemon counts as stale. |

### Running `serve` as a service

A systemd user unit ships at `rs/merge-queue/merge-queue-serve.service` (`Restart=on-failure`,
`ExecStart` pointing at `serve`). Edit its `WorkingDirectory` and `ExecStart` paths, then:

```bash
mkdir -p ~/.config/systemd/user
cp rs/merge-queue/merge-queue-serve.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now merge-queue-serve
merge-queue status   # daemon: running ...
```

The tick-hub launcher alternative ships at `rs/merge-queue/tick-hub-merge-queue.yaml`: each tick
runs `merge-queue status` as a gate and emits an action asking for a restart when the daemon is
not healthy, plus a heartbeat-freshness health check. Edit its paths, then
`tick-hub tick --config rs/merge-queue/tick-hub-merge-queue.yaml`.

### `merge-queue serve`

This command runs the queue forever: deterministic code that is always running, so agents submit
changes and never drive the queue loop by hand. While idle it holds **no** writer lock. It polls
the `items/` directory fingerprint every `--poll-ms` (default `250`) and polls the remote base
with a cheap `git ls-remote` every `--base-poll-secs` (default `30`). A changed fingerprint (a
new `submit`, `sync` or `withdraw`) or a moved base triggers one work cycle: the daemon takes the
writer lock, plans, and runs the same `Runner` path as `run` (the logic is not forked), then
releases the lock and goes back to idle polling. A cycle that finds the lock held, a refused
validation, or a failed base poll is recorded in the heartbeat and retried on the next trigger;
it never kills the daemon. Stop it with SIGINT or SIGTERM (Ctrl-C); a graceful stop removes the
pid file, a crash leaves it behind for `status` to report.

| Option | Default | Meaning |
|---|---|---|
| `--poll-ms N` | `250` | Idle poll interval in milliseconds: `items/` watch and heartbeat cadence (at least 10). |
| `--base-poll-secs N` | `30` | Seconds between `git ls-remote` base polls. `0` polls the base every cycle. |
| `--once` | off | Run at most one work cycle, then exit. For scripts and tests. |
| `--depth N` | the configured `depth` | Override the initial window for work cycles. |
| `--max-batch N` | the configured `max_batch` | Override the batch size for work cycles. |

### `merge-queue status`

This command reports daemon liveness, the current window and slot occupancy without taking the
writer lock. Liveness comes from `serve.pid` and `serve.heartbeat.json` in the state directory
(the daemon rewrites the heartbeat every cycle): `running` (pid alive, heartbeat fresh), `not
running` (no pid file), `dead` (pid file names a dead process), `stale` (pid alive but the
heartbeat is older than `--stale-after-secs`), or `malformed` (pid file unreadable). Every
non-running verdict names its remedy. The window is the daemon's active batches (or the newest
run's plan when no daemon heartbeat exists); slots count the `scratch/slot-*` checkouts.

| Option | Default | Meaning |
|---|---|---|
| `--json` | off | Print the status report as JSON (`daemon`, `pid`, `heartbeat`, queue counts, `active_batches`, slots, `remedy`). |
| `--stale-after-secs N` | `10` | Heartbeat age in seconds after which a live daemon counts as stale. |

### Running `serve` as a service

A systemd user unit ships at `rs/merge-queue/merge-queue-serve.service` (`Restart=on-failure`,
`ExecStart` pointing at `serve`). Edit its `WorkingDirectory` and `ExecStart` paths, then:

```bash
mkdir -p ~/.config/systemd/user
cp rs/merge-queue/merge-queue-serve.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now merge-queue-serve
merge-queue status   # daemon: running ...
```

The tick-hub launcher alternative ships at `rs/merge-queue/tick-hub-merge-queue.yaml`: each tick
runs `merge-queue status` as a gate and emits an action asking for a restart when the daemon is
not healthy, plus a heartbeat-freshness health check. Edit its paths, then
`tick-hub tick --config rs/merge-queue/tick-hub-merge-queue.yaml`.

### `merge-queue widen`

This command runs the complete suite (`all`) on the current base. It needs a validator but no
land driver. A green run discharges every open obligation whose landed commit is contained in that
base. A red run exits `3` and discharges nothing: each covered obligation stays open and records the
run (commit and failed tests) in its `failed_runs`. The focused landings since the last green
complete run are the suspects; the queue reports and does not revert. `--json` prints the report as
JSON.

### `merge-queue obligations`

This command lists widening obligations: id, landed commit, changes, what they were validated
with, and status (`open`, or discharged green). An open obligation whose complete run was red
shows that run. `--all` includes discharged ones; `--json` prints the records.

### `merge-queue bench`

This command builds a throwaway bare remote with `--prs` independent changes (a seeded fraction
fail their test) and a validator costing `--test-secs` per run, then drains it twice from scratch:
serially (batch 1, depth 1, the complete suite every time) and with the queue's settings (batches,
the window, focused selection). It prints changes landed per hour for each and the speedup.

| Option | Default | Meaning |
|---|---|---|
| `--prs N` | `16` | Changes in the synthetic queue. |
| `--fail-rate F` | `0.125` | Fraction of changes that fail their test. |
| `--test-secs S` | `1.0` | Wall seconds every validation costs (stands in for the real validation time). |
| `--depth N` | `2` | The queue's window. |
| `--max-batch N` | `4` | The queue's batch size. |
| `--seed N` | `1` | Picks the failing changes. |
| `--keep DIR` | a deleted temporary directory | Keep the synthetic repositories in `DIR`. |
| `--json` | off | Print JSON instead of a table. |

A measured example (8 changes, 1 failing, 0.5 s per validation):

```text
config   depth max-batch selection  landed  ejected validations   wall-s  changes/h
serial       1         1       all       7        1          10     10.0     2512.5
queue        2         4   focused       7        1           8      7.1     3533.1
speedup: 1.41x
```

The figures depend on validation time, failure rate and depth. Even the serial lander pays extra
runs for a culprit, because a blamed change must fail twice and the base is probed.

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
| `validation-refused` | None: validation admission refused to run. The change stays queued and is retried after its backoff (`next_retry_at`). |

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
  serve.pid              fixed: `serve` daemon pid (removed on a graceful stop)
  serve.heartbeat.json   fixed: `serve` heartbeat, rewritten every cycle
  serve.pid              fixed: `serve` daemon pid (removed on a graceful stop)
  serve.heartbeat.json   fixed: `serve` heartbeat, rewritten every cycle
  union.attributes       fixed: generated from union_paths, when set
  items/<pr>.json        one record per submitted change
  notifications/        notification report files the daemon sends with `--file`
  notifications/        notification report files the daemon sends with `--file`
  runs/<run-id>/         one directory per run (layout below)
  obligations/<id>.json  one record per focused landing still owed a complete run
  scratch/slot-<n>/      one reused worktree per concurrent validation
```

A run directory holds `plan.json` (the plan it started from), `events.jsonl` (every step, in
order), `validations/<NNNN-purpose>/` (one per validation: `run.log`, the runner's results and
`record.json`), `ejections/<pr>.md` (one report per ejected change), `report.md` and
`report.json`. The scratch slots are kept between validations and runs: switching a slot to a new
candidate is a forced checkout plus `git clean -ffd`, which keeps ignored build caches, so later
builds are incremental (`validator.clean_ignored` adds `-x`). There is one slot per possible
concurrent validation, plus one for selection queries.

Growing collections live in their own subdirectories. Records are written atomically (temporary
file, then rename). `submit`, `withdraw`, `run` and `widen` take the writer lock and fail fast if
another writer holds it.

An **item record** (`items/<pr>.json`) has these fields:

- `pr`, `head_ref`, `head`, `priority`, `submitted_at`, `submitted_by`, `notify`, `notify_delivery`, `title`, `after`, `solo`.
- `claim`: `{selection, policy, scope, commit}`. `selection` is `{"kind":"all"}` or
  `{"kind":"tests","tests":["step","step::case"]}`.
- `status`: `queued`, `landed`, `ejected` or `withdrawn`.
- `landed_as`: the base commit that contains the change, once it has landed.
- `validation_refusals`, `next_retry_at`, `last_refusal`: present only after a validation
  refusal. The status stays `queued`; `next_retry_at` is when validation may be retried.
- `history`: a list of `{at, kind, detail}` events. A refusal adds a `validation-refused` event.

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
| `depth` | `2` | The initial window and the culprit-search probe width. |
| `max_depth` | `0` | The largest adaptive window; `0` means fixed at `depth`. |
| `union_paths` | `[]` | Attribute patterns merged with Git's union driver (see "Union merges"). |
| `regenerate` | empty | `{paths, command, when}`; see "Generated files". |
| `exact_sha` | `false` | The exact-commit rule. |
| `trust_base` | `false` | Skip probing the base in culprit searches. |
| `classify` | `[]` | `[{pattern, class}]` failure classes; see `--classify`. |
| `peers` | `{}` | Peer queue name to state directory. |
| `identity` | empty | `{name, email}` for commits the queue creates. |
| `github` | empty | `{repo, gh, max_per_hour, budget_file, anonymous_fetch}`; see "Ready sources and the request budget". |
| `validator` | empty | How validation runs; see below. |
| `land` | `push` from `init` | How a validated candidate reaches the base; see below. |
| `notify` | empty | `{agentctl}` command words for state-change notifications (see "Notifications"). |

**`validator`** fields (the `init` flags of the same names set them):

- `kind`: `command` (a shell template) or `dagrun` (a DAG file, with step-level results).
- `list`: prints the complete inventory (recorded; not yet used by `run`).
- `select`: prints the policy's selection for a candidate, given `$MQ_BASE` and `$MQ_HEAD`. Empty
  means the policy is `all`.
- `id_prefix`: only output lines with this prefix are test IDs.
- `policy`: the name recorded for the selection policy.
- `run`: runs a selection.
- `dagrun`, `dag`: the executable and the DAG file, for the `dagrun` kind.
- `admission`: a command prefix that every validation runs under, for example a host admission
  wrapper that limits concurrent validations.
- `refusal_max_retries`: extra attempts after an admission refusal within one run (default `3`).
- `refusal_backoff_secs`: base backoff between refusal retries, doubled per attempt and capped
  at 60 seconds (default `2`; `0` means no delay).
- `timeout_secs`: `0` means no limit.
- `clean_ignored`: also delete ignored files from reused checkouts (default `false`).

**`land`** fields:

- `driver`: `push` or `command` (see "Land drivers"). Any other value makes `run` refuse to start
  unless it is a `--shadow` or `--dry-run` run.
- `command`: the shell template for the `command` driver.
- `allow_batches`: for the `command` driver, allow batches of several changes (default `false`,
  which makes every batch one change).

## Validators

Every validator takes a selection and returns per-test results. Both kinds share the selection
policy: `select` runs in a checkout of the candidate with `$MQ_BASE` (the candidate's parent) and
`$MQ_HEAD` set, and prints test IDs, one per line (only lines starting with `id_prefix` count; a
line `all` selects everything). Without `select`, every validation is complete. Every validation
runs in the candidate checkout, under the `admission` prefix, with the `timeout_secs` limit.

**`command`.** `run` is a shell template (`sh -c`) that reads the selection from:

- `$MQ_SELECTION`: `all` or `tests`;
- `$MQ_TESTS`: the IDs, space separated (empty for `all`);
- `$MQ_TESTS_FILE`: the IDs, one per line;

and writes one JSON object per line to `$MQ_RESULTS`:
`{"id": "step::case", "outcome": "pass|fail|skip|error", "duration_ms": 12}`. A worse outcome
for the same ID wins. Selected tests it never reports count as errors. A command that writes no
results still works, but then its exit status is the only signal: every selected test inherits it,
and the result is marked `unattributed`. A culprit search over unattributed results can only rerun
the batch's whole selection, so per-test results are what make focused searches possible. A
non-zero exit with no failed test reported counts as one failure of `(unattributed)`.

A complete runner over a directory of shell tests:

```sh
#!/bin/sh
# run-selected.sh: each file tests/NAME.sh is test NAME.
if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests | sed 's/\.sh$//'); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  start=$(date +%s%3N)
  if sh "tests/$t.sh" >"/tmp/$t.log" 2>&1; then o=pass; else o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s","duration_ms":%s}\n' "$t" "$o" $(( $(date +%s%3N) - start )) >>"$MQ_RESULTS"
done
exit $rc
```

with `merge-queue init --validator command --validator-run 'sh run-selected.sh' --select
'git diff --name-only "$MQ_BASE" HEAD -- tests | sed -n "s|^tests/\(.*\)\.sh$|\1|p"' --policy
path-routing`.

**Admission refusals (retry later).** A validation that admission refuses to run at all — the
runner exits `75`, or its combined output contains `COULD_NOT_RUN` — did not run, so it is
neither a pass nor a failure. The queue records the validation as `refused` with no failed
tests, retries it in the same run after a backoff (`refusal_backoff_secs` doubled per attempt,
up to `refusal_max_retries` extra attempts), and if it is still refused, defers the batch: every
change in it stays `queued`, its `validation_refusals` count grows, `next_retry_at` is set from
the backoff, the run report lists it under `deferred`, and the run stops with exit `3`. No
culprit search runs and nothing is ejected for a refusal. While `next_retry_at` is in the
future, `plan` holds the change as `validation-refused`; after it passes, the next `run`
validates it again. A refusal in a culprit-search probe or a widening run defers the same way
(no culprit is named, no obligation is discharged or marked red).

**Verdict lines.** A `command` runner that writes no results file but prints dagrun-style verdict
lines (`[step] ✓ PASS ...`, `[step] ✗ FAIL ...`, `[step] SKIPPED ...`) is still attributed per
step from its output.

**Same-commit reruns.** A runner may write an opaque handle for its run (a run ID, a log path)
to `$MQ_HANDLE_FILE`. When the queue later validates the same commit again, to rerun failed tests
in a culprit search, it passes the latest handle as `$MQ_RERUN_FROM` and that run's directory
as `$MQ_RERUN_OF`. A runner that can rerun only the failed parts of an earlier run at the same
commit uses them for a cheap targeted rerun. If that rerun is green, the failure was flaky: the
batch lands with the test recorded under `flaky`, and the exact-commit rule counts the same-commit
rerun as coverage. For example:

```sh
if [ -n "$MQ_RERUN_FROM" ]; then my-runner --rerun-failed-from "$MQ_RERUN_FROM"
else my-runner --target "$(git rev-parse HEAD)" full; fi | tee /dev/stderr |
  grep -o 'run-[0-9a-f]*' | head -1 > "$MQ_HANDLE_FILE"
```

A runner that can only run everything at a new commit still works. Culprit-search probes at other
commits then cost full runs, so keep batches small (`--max-batch 1` or `2`). With `--trust-base`,
a red single-change batch costs exactly one targeted rerun.

**`dagrun`.** Test IDs are DAG step names. A selection becomes `dagrun run --dag DAG --keep-going
--selected a,b`, which also runs each selected step's dependencies; `all` omits `--selected`. The
per-step verdicts come from the run journal (`journal.jsonl` under a private `DAGRUN_LOG_DIR`):
`step_end` gives pass or fail with the wall time, `step_skip` gives skip. A case-level ID narrows
nothing for this kind; its whole step runs.

## Using it on a repository validated by a dagrun DAG

```sh
merge-queue init --validator dagrun --dag validation.dag.yaml --dagrun common/bin/dagrun \
  --select 'python3 scripts/validate.py --base "$MQ_BASE" --list' --id-prefix 'STEP ' \
  --policy path-routing \
  --admission python3 --admission scripts/gate.py --admission=--
```

- `--validator dagrun --dag validation.dag.yaml --dagrun common/bin/dagrun`: each step of the
  repository's validation DAG is one test, run by the repository's own dagrun launcher in the
  candidate checkout.
- `--select '... validate.py --base "$MQ_BASE" --list'`: the repository's path-routing selector
  lists the steps a change from `$MQ_BASE` to HEAD needs, one `STEP <id>` line each, after closing
  over dependencies. `--id-prefix 'STEP '` keeps those lines and drops the rest of its report.
- `--policy path-routing`: the name every focused claim records.
- `--admission python3 scripts/gate.py --`: every validation waits for a slot in the host's
  shared gate admission queue, so the queue's concurrent validations count against the same host
  limit as everyone else's.

A candidate's selection is computed against its parent, so each batch in the window is selected
for its own changes only.

## Land drivers

- **`push`.** The remote base is fast-forwarded to the exact validated candidate with a plain
  (non-force) push. A rejected push means the base moved: the batch is rebuilt on the new base and
  validated again. After the push, the queue fetches the base and requires it to equal the
  candidate. For a local repository (`--remote ''`) the base branch is moved with a
  compare-and-swap `git update-ref`.
- **`command`.** The template runs once per change, in order, in the repository directory, with:

  | Variable | Value |
  |---|---|
  | `MQ_PR` | the change number |
  | `MQ_HEAD_REF`, `MQ_HEAD` | the submitted head ref and commit |
  | `MQ_PREFIX` | the validated candidate commit after this change |
  | `MQ_EXPECTED_TREE` | the tree of `MQ_PREFIX` |
  | `MQ_PARENT` | the base commit before this change |
  | `MQ_BASE`, `MQ_REMOTE` | the base branch and the remote |

  After each change the queue fetches the base and requires its tree to equal `MQ_EXPECTED_TREE`.
  If it differs, or the command fails, the queue stops and says so. Changes that had already landed
  are recorded `landed`, with a note that the batch landed only partly. Before starting, an
  unchanged base is recognised by its tree rather than its commit, because a hosted rebase merge
  rewrites commit IDs.

  Because each change reaches the base on its own, the base passes through every intermediate
  tree of a batch. Only the batch's last tree was validated, so this driver lands one change per
  batch unless `land.allow_batches` (`init --allow-batches`) says those intermediate trees are
  acceptable. The window still validates the next change on top of this one at the same time.

The simplest `command` driver pushes each prefix:

```sh
merge-queue init --land command \
  --land-command 'git push -q "$MQ_REMOTE" "$MQ_PREFIX:refs/heads/$MQ_BASE"'
```

A lander script that rebase-merges one pull request through the hosting service plugs in the same
way (`--land-command 'my-lander --pr "$MQ_PR" --expect-tree "$MQ_EXPECTED_TREE"'`): it should push
`MQ_PREFIX` to the change's branch, merge exactly that head, and fail rather than retry.

## Keeping the base green

The base moves only to a commit whose exact tree passed validation (the bors rule).

1. **The rolling window** (a dependent pipeline, as in Zuul). Batches are built as a chain: the
   first on the base, each next one on the candidate before it, and up to *window* of them validate
   at once. Green batches land in order, as soon as every batch ahead of them has landed, and the
   next batch starts at once on top of the last one in flight.
2. **AIMD.** The window starts at `depth`, grows by one per landed batch up to `max_depth`, and
   halves per red batch, never below one. With `max_depth` 0 it stays at `depth`. The report's
   `windows` field records the window each time a batch started.
3. **A red head** cancels everything behind it: those candidates were built on its tree. Their
   validations are killed, reported as `cancelled` (never as red), and their batches go back to the
   front of the queue, to be rebuilt once the red batch is resolved.
4. **Culprit search.** The queue searches the red batch's prefixes (`parent + first i changes`) for
   the first one that fails, running **only the failed tests**. Each round probes up to `depth`
   prefixes at once, evenly spaced (a W-ary search, not halves). The bare parent is probed until it
   is known green. A prefix search respects stacked changes and also catches a change that fails
   only in combination with the ones before it.
5. **Seen twice.** Every blaming conclusion needs its failure observed twice: a culprit's prefix
   must fail twice (with the prefix before it passing), and a base that fails the tests twice stops
   the queue (exit `3`) instead of ejecting an innocent change. Spare probe slots rerun the
   suspected failing prefix in the same round, so a single-change batch costs one round of two
   concurrent runs (the base and a rerun). A prefix that fails and then passes is flaky: its tests
   are recorded and the search continues. If the full candidate passes on rerun, the batch lands
   with the tests recorded under `flaky` in the report. Flaky results are never hidden.
6. **Ejection.** The culprit is marked `ejected`, with a report in `runs/<id>/ejections/<pr>.md`
   naming the failed tests. Changes that depend on an ejected change (ejected for failed tests or
   for a conflict) are set aside for the rest of the run and stay `queued`; they never land in that
   run, and the next plan holds them as waiting on an ejected dependency.
7. **The remainder** is rebuilt without the culprit and revalidated with a delta selection: the
   failed tests; every test the batch run did not see pass (skipped, errored or never reported);
   and the tests the selection policy says the culprit's change affected, within the batch's own
   selection. Tests outside the delta passed on the batch's tree and are carried forward. A skipped
   test cannot be: a runner often skips a test because something it depends on failed, so the
   batch run says nothing about it, and a change hidden behind the culprit's failure would
   otherwise land untested. The landing records scope `focused` with policy
   `carried-forward+delta(<policy>)` and leaves an obligation. If the remainder fails, the search
   repeats on it; each round ejects one change, so it terminates.
8. **A moving base.** Landing re-checks the base. If it moved, the batch (what is left of it after
   ejections) and everything behind it is rebuilt on the new base and validated again.
9. **Obligations.** Every focused landing leaves an obligation until a green complete run
   (`widen`, or `run --widen`) covers that commit or a later one. A red complete run leaves it open.

## Changes across repositories

When a change in one repository needs a change in another to land first, configure the other
repository's queue as a peer. One example is an application whose validation requires its pinned
library commit to be on the library's main branch. Then declare the requirement when submitting:

```sh
merge-queue --state lib/.git/merge-queue submit 7 --head refs/pull/7/head     # in the library's queue
merge-queue init --peer lib=/path/to/lib/.git/merge-queue ...                 # in the application's
merge-queue submit 41 --head refs/pull/41/head --requires lib#7
merge-queue run --with-peers
```

- **`--requires lib#7`** records the exact head of the library's change #7. It also marks that
  change exact: it lands only as its own commits (a fast-forward, in a batch of its own), so the
  pinned commit appears on the library's base. If the library's base has moved on, the exact
  change is ejected (kind `not-fast-forward`) rather than replayed under a new commit ID.
- **Until the required commit is on the peer's base,** the plan holds the dependent change
  (`waiting-on-peer`). The hold says whether the peer change is queued (and `--with-peers` will
  land it first), ejected or withdrawn (so it will not land at that commit), or unknown.
- **`run --with-peers`** drains each peer queue that holds a needed change, then plans and runs
  this queue. Dependents whose requirement landed proceed; the others stay queued.
- **The requirement is checked again right before landing,** so a dependent never lands without
  it.
- **A shadow run drains its peers in shadow too.** Their changes do not really land, so dependents
  stay held, and the shadow report says why.

## Failure classes

Some failures are of a known kind, and naming the kind tells the author what to do. With
`--classify PATTERN=CLASS`, every failed test whose ID (or step) matches a pattern is labelled
with that class. The label appears in three places:
- the ejection record (`classes`);
- the ejection report file (`Failure class: ...`), the run report and the item's history;
- the validations table, next to red results.

Classification changes nothing about the outcome. An ejected change is still ejected. A typical
use is a pinned count, such as the number of tests in a suite. Two changes that each add a test
and pin the count they saw merge cleanly, but together they break the pin. The search finds the
second change; it is ejected as `count pin drift` (its pin must be recomputed on the new base),
and the rest of the batch lands.

## Ready sources and the request budget

Planning, validating and landing make no API calls. Heads arrive in one batched `git fetch` of
`refs/pull/N/head` (or of branches), and landing is a push. The hosting service's API is used
only by `sync --label` and `sync --all-open`, one list call for the whole queue, and by
`--close-prs`, one call per landed pull request.

Every API call is charged to a budget first: `--api-per-hour` requests per rolling hour, kept in
`api-budget.json` or in a shared `--api-budget-file`. A call the budget cannot cover is refused
before it is made, and the error says when room opens up.

**Which ready source to use.**
- **`submit`.** When the agents that finish changes run on the same host as the queue, they should
  call `merge-queue submit` directly. It costs no API request, binds the exact head, and records
  which tests the agent ran.
- **`sync --label`.** Use it when people mark changes ready on the hosting service, or when you
  want the ready set visible there. It costs one request per sync, plus one per label edit.
- **`sync --file`.** Use it for a ready list kept by other tools. It costs no API requests.
- **`sync --all-open`.** For shadow runs and measurements.

For public repositories, `--anonymous-fetch` keeps fetches from spending credential uses at all.

## Generated files

Some repositories commit files generated from sources, such as a validation graph compiled to
JSON or a static table compiled to code. Two changes that each regenerated such a file conflict
on it, even when their sources combine cleanly. If those files cause most of a backlog's rebases,
list them with `--regen-path` and give the command that rebuilds them with `--regen-command`.

- **Planning.** A conflict confined to generated files is not a conflict. The plan batches those
  changes together instead of holding all but one.
- **Building.** When combining a change leaves conflicts only in generated files, the queue checks
  the combination out in a scratch worktree, runs the command there, and commits the rewritten
  files on top as `merge-queue: regenerate <paths> after #<pr>`, under `identity`. It does this
  right after that change, so every prefix the culprit search might probe is consistent. With
  `--regen-when always`, it also regenerates after every other change and commits only when a
  generated file actually changed.
- **Validation.** The candidate is then validated as usual. The commit that lands is the exact
  validated commit, regeneration included.
- **Refusals.** A change is ejected with kind `regeneration` when the command fails, when it
  changes any file that is not generated, or when conflict markers remain afterwards.

This is deterministic, not repair. The command must give the same output for the same sources.
The run report lists every regeneration under `regenerated`.

## The exact-commit rule

With `--exact-sha`, no validation result is ever carried to a different commit:

- A remainder left after an ejection reruns the batch's whole selection, not a delta.
- Before any landing, the queue checks that it recorded green coverage of the claimed selection
  at exactly the commit being landed. A rerun of failed tests at the same commit counts, which is
  how flaky passes work. If it finds no such coverage, it refuses to land and stops.
- A batch rebuilt because the base moved is validated again in full. It always was; the rule
  makes that explicit.

In rebase mode, a change whose head already sits on the base with linear history lands with its
own commits, exactly as `git rebase` would leave it. So a change rebased onto the current base
by its author lands with its own commit ID.

## Union merges

`union_paths` (`init --union-path`) names files merged with Git's built-in `union` driver, which
keeps both sides' lines instead of reporting a conflict. The queue writes the patterns to
`<state>/union.attributes` and passes `-c core.attributesFile=<that file>` to every Git command, so
conflict probes and candidate building agree. It is opt-in per path, for line-oriented files that
are only ever appended to (lists of tests, changelog-style indexes). On anything else union
silently produces duplicated or resurrected lines and broken syntax; validation still checks the
result, but the change is then ejected. A study of 400 commits of this repository found no conflict
that union would have removed, and its most frequently edited files mix deletions with in-place
edits, which is why union is never on by default.

## Run report

`report.md` and `report.json` give the `mode` (`run`, `shadow`, `dry-run` or `widen`), the base
before and after, what landed (changes, commit, the claim it was validated with, flaky tests, and a
`note` for a partly landed batch), what was ejected and why, changes returned to the queue, new
obligations, the batches a dry run `would_validate`, and a table of every validation: purpose
(`batch`, `probe`, `remainder`, `widen`), changes, commit, selection and policy, result (green,
red with the failed tests, or `cancelled`), `start_ms` (since the run began) and `elapsed_ms`. The
summary gives wall time, total validation time, `max_concurrent` (the most validations running at
once), the AIMD `windows` trace and `prs_per_hour` (changes landed per hour of wall time).
