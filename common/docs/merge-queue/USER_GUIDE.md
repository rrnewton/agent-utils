# merge-queue user guide

merge-queue is a local merge queue for one base branch of one Git repository. You submit
changes as ready to land. The queue orders them, splits them into batches, validates each batch's
exact combined tree, and lands only trees that passed. It runs where your clone is. There is no
hosted service, no webhook and no inbound connection.

Commands: `init`, `submit`, `withdraw`, `retry`, `list` and `plan` decide what can land; `run` builds,
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
with a fresh heartbeat and every item record reads, and `1` when it is not running, dead,
stale or malformed, or an item record is unreadable (the output names the verdict and the
remedy). Exit codes 126 and 127 are never success. `status` exits `0` only when the daemon is running
with a fresh heartbeat and every item record reads, and `1` when it is not running, dead,
stale or malformed, or an item record is unreadable (the output names the verdict and the
remedy). Exit codes 126 and 127 are never success.

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
| `--priority-label LABEL` | `mq-priority` | The label that makes a pull request a priority change when `sync` reads it from the hosting service, compared without regard to case. `-` turns it off. See "Priority changes". |
| `--classifier-policy PATH` | none | A test-footprint policy in the repository (version 2, such as `ci/test-footprints.json`), read at each plan's base commit. Decision-log features then take their components and a verdict from it, through the shared change classifier (`change-classify userguide`). |
| `--ci-path PATTERN` | none | A CI or validation-infrastructure path (exact path, directory with or without a trailing slash, or `*` wildcards). `plan` suggests the priority label for a change that touches only these paths; the queue never applies the label itself. Repeatable. |
| `--regen-path PATTERN` | none | A generated file (exact path, directory with or without a trailing slash, or `*` wildcards) whose conflicts the queue settles on the target's side and then regenerates, instead of holding the change. Repeatable. See "Generated files". |
| `--regen-command CMD` | empty | Shell command, run as `sh -c` at the root of the candidate checkout, that rewrites every generated file from its sources deterministically. Runs with `MQ_PR` (change number), `MQ_REGEN_PATHS` (space-separated generated patterns) and `MQ_CONFLICTED` (space-separated conflicted paths, empty for a non-conflict refresh) set. Required with `--regen-path`. |
| `--regen-when WHEN` | `conflict` | `conflict`: regenerate after a change whose combination conflicted in, or changed, a generated file (a textual merge of generated files need not be the regenerated output). `always`: regenerate after every change. Either way regeneration happens per change, so every prefix is a self-consistent tree. |
| `--regen-build CMD` | empty | Command run before `--regen-command`, such as building the generator. When it fails, the change is ejected as `regen-build-failed` with the error tail. |
| `--exact-sha` | off | Exact-commit rule: never carry a validation result to another commit. See "The exact-commit rule". |
| `--trust-base` | off | Treat the base as green in culprit searches and skip probing it. Sound when only this queue lands on the base, always as exactly validated commits; a broken base would then be blamed on the first change of a red batch. |
| `--classify PATTERN=CLASS[=EVIDENCE]` | none | Label failed tests whose ID, or whose step alone, matches `PATTERN` (exact or `*` wildcards) with `CLASS` in ejection records, reports and item history. With `EVIDENCE` (text with `*` wildcards), only when a line of the validation log tagged with the test's step (`[step] ...`) contains it. One example is `--classify 'test.*_count=count pin drift=expected * tests to execute, saw *'`. Repeatable. |
| `--recount-pins` | off | Recount the generated count pins (`ci/manifest-plan/src/validation_dag_static.rs`) after merging each change instead of treating their conflicts as real: each `("name", count)` entry becomes the base value plus every batched change's own delta (head minus its merge base), matched per name and occurrence. A pin a change introduces (absent at its merge base and at the base) takes the head's value; a pin the change leaves as it was keeps whatever the base did to it since, even one the base added or removed. A pin the change lowers (head below its merge base) lands only when the change's head commit declares exactly that decrease with a `Count-Pin-Decrease: <pin> <amount> <reason>` trailer (`<pin>` is a pin name for its first occurrence, or `name:N` for occurrence `N`; `<amount>` is written `2` or `-2`; one trailer per pin); an accepted declaration is recorded in the change's history (`count-pin-decrease`) and in the run report (`pin_decreases`). A change is skipped from its batch and stays queued when its pins cannot be recounted (the file missing in a version, a pin it changes whose occurrence count differs across versions, or a decrease that is undeclared or differs from its declaration; that refusal names the pin's merge-base and head values, so a removed test can be told from a stale pin, and the trailer that would declare it). A change whose new pin is lost settling a conflict in the pin file is ejected as a conflict naming that pin. |
| `--peer NAME=DIR` | none | A peer queue: another repository's merge-queue state directory. Repeatable. See "Changes across repositories". |
| `--identity-name NAME`, `--identity-email EMAIL` | Git's configuration | Identity for commits the queue creates: committer of replayed commits, author and committer of merge and regeneration commits. |
| `--github-repo OWNER/NAME` | empty | Hosting repository for `sync --label`, `sync --all-open` and `--close-prs`. |
| `--gh WORD` | `gh` (or `$FLEET_REAL_GH`) | Command (and leading arguments) to run instead of `gh`, for example a pacing wrapper. Repeatable. When no `--gh` is configured, the `FLEET_REAL_GH` environment variable (shell-style words naming the real `gh` on fleet hosts where `gh` on `PATH` may be a stub) is used as a fallback; an explicit `--gh` always wins. |
| `--api-per-hour N` | `0` (= 60) | API requests this queue may make per rolling hour. See "Ready sources and the request budget". |
| `--api-budget-file PATH` | state directory | Budget file to share with every other queue on the host, so they all draw on one hourly allowance. |
| `--anonymous-fetch` | off | Fetch without credentials (public repositories). Credential helpers are disabled for fetches, so a fetch never spends a credential use. |
| `--require-approval-at-head` | off | For `sync --label`/`sync --all-open`: a pull request counts as ready only when a reviewer with write access (author association `OWNER`, `MEMBER` or `COLLABORATOR`) has, as their latest verdict, approved exactly its current head commit (full commit id; comments do not replace a verdict), no such reviewer's latest verdict requests changes, and the hosting service's `reviewDecision` is not `CHANGES_REQUESTED` or `REVIEW_REQUIRED`. Missing review data never approves. Reviews are read in the same batched list call (no per-pull-request calls), charged 3 requests per hundred results instead of 1 because of the nested review connections. An unapproved change is not submitted; a queued change whose head moves without a fresh approval at the new head is withdrawn and its submitter notified (an existing `--notify` subscription is kept; the author is only the default). Resubmission with an approval at the new head re-queues it. Ready-file sources are unaffected. |
| `--require-approval-at-head` | off | For `sync --label`/`sync --all-open`: a pull request counts as ready only when a reviewer with write access (author association `OWNER`, `MEMBER` or `COLLABORATOR`) has, as their latest verdict, approved exactly its current head commit (full commit id; comments do not replace a verdict), no such reviewer's latest verdict requests changes, and the hosting service's `reviewDecision` is not `CHANGES_REQUESTED` or `REVIEW_REQUIRED`. Missing review data never approves. Reviews are read in the same batched list call (no per-pull-request calls), charged 3 requests per hundred results instead of 1 because of the nested review connections. An unapproved change is not submitted; a queued change whose head moves without a fresh approval at the new head is withdrawn and its submitter notified (an existing `--notify` subscription is kept; the author is only the default). Resubmission with an approval at the new head re-queues it. Ready-file sources are unaffected. |
| `--close-prs` | off | After landing, close each landed pull request with a comment naming the landed commit. Uses the budget; a close it cannot make now is retried by `sync`. |
| `--agentctl WORD` | `agentctl` | Command (and leading arguments) the `serve` daemon runs instead of `agentctl` for state-change notifications. Repeatable. |
| `--agentctl-registry PATH` | none | agentctl registry the notifications address: sends run `<command> --registry PATH send ...`. Stored absolute as `notify.registry`. Without it agentctl uses `.agentctl` in the daemon's working directory, which for a daemon started in a bare queue repository is no registry, so every send fails. |
| `--net-wrapper WORD` | none | A command prefix for network Git operations, for example `--net-wrapper with-proxy`. Repeat it for a prefix of several words. |
| `--validator KIND` | empty | `command` or `dagrun` (see "Validators"). `run` refuses to start without one. |
| `--validator-run CMD` | empty | For `command`: the shell template that runs a selection. |
| `--select CMD` | empty | Shell command printing the selection policy's test IDs for the checkout's HEAD relative to `$MQ_BASE`. Empty: every validation runs the complete suite. |
| `--id-prefix TEXT` | empty | Only lines of `--select` output that start with this prefix are test IDs (the prefix is removed), for example `STEP `. |
| `--policy NAME` | empty | The name recorded for the selection policy (empty records `select-command`, or `all` without `--select`). |
| `--dag FILE` | empty | For `dagrun`: the DAG file, relative to the candidate checkout. |
| `--dagrun PATH` | empty | For `dagrun`: the dagrun executable, relative to the checkout or absolute (empty means `dagrun` on `PATH`). |
| `--admission WORD` | none | A command prefix every validation runs under, such as a host admission wrapper. Repeat it for several words. |
| `--refusal-max-retries N` | `3` | Extra attempts after an admission refusal (exit `75`, or a non-zero exit whose first output line starts with `COULD_NOT_RUN` and that attributed no failed test) within one `run`, each after a backoff delay. A refusal is never a pass and never red: the change stays queued. |
| `--refusal-backoff-secs SECS` | `2` | Base backoff between refusal retries, doubled per attempt and capped at 60 seconds. `0` means no delay. The same backoff sets the deferred change's next retry time. |
| `--max-deferrals N` | `3` | How many times a change may be deferred because validation did not run (an admission refusal, a validator that cannot execute, a probe or donors that could not run) before it is escalated to its owner and held until `merge-queue retry` (see "Bounded retry"). |
| `--green-flaky-hold-secs SECS` | `7200` | How long a change is held for a ruling after a GREEN_FLAKY or INFRA-BLOCKED result (see "Held for a ruling") before it is re-queued for a fresh validation. |
| `--red-without-probe MODE` | `eject` | What a red validation that names failed tests does when its follow-up probe cannot run: `eject` the change (the red run is the verdict) or `hold` it for its owner. |
| `--offload MODE` | `off` | Where validations run: `off` (in this process, as always), `compare` (in this process, which decides, and also on donors, whose receipts are only compared) or `decide` (on donors only; an accepted receipt is the verdict). See "Donor validators". |
| `--tool-identity NAME=COMMAND` | none | A tool the validation depends on (repeatable): the command prints its identity, for example a tool repository's commit or a toolchain version, never a hash of a binary (builds are not byte-identical across hosts). A donor whose answer differs does not run the job (see "Tool identity and receipts"). merge-queue's own revision is always compared. |
| `--tool-refresh NAME=COMMAND` | none | How a donor brings the tool `NAME` (one of `--tool-identity`) up to date, for example a fetch of the tool repository (repeatable). A donor whose answer differs runs it once and asks again: an older donor then takes the job, and one that still differs declines it. |
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
and sets its status back to `queued`. While a run holds the writer lock, the submission is
recorded instead and applied when the lock frees (see "Intake while a run holds the queue").

| Argument | Default | Meaning |
|---|---|---|
| `PR` | required | The change number (pull-request number). It must be positive. |
| `--head REF` | required | The head branch on the remote, or a full ref such as `refs/pull/42/head`. |
| `--sha SHA` | empty | The exact head commit (or a prefix of it) that you mean. Submission is refused if the branch points elsewhere. |
| `--priority N` | `100` | Lower values land first. |
| `--after PR` | none | A change that must land first. Repeatable. |
| `--solo` | off | Validate and land this change in a batch of its own. Use it for risky changes. |
| `--prioritized` | off | A priority change, as the priority label makes one under `sync`: planned ahead of all other work and validated in a batch of its own. It still lands only on its own exact green. See "Priority changes". |
| `--by NAME` | empty | Who is submitting (an agent or a person). |
| `--notify NAME` | empty | agentctl session name the `serve` daemon notifies exactly once per state change (landed, ejected, refusal-deferred, escalated, withdrawn). Stored on the item next to `submitted_by`; resubmitting without it keeps the recorded name. Ready files set it per line with `notify=NAME`. |
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

**Label scope (`sync --label`).** The labelled ready set is the only set a label sync touches:
one batched list call returns open, non-draft pull requests carrying the label (charged one
request per hundred listed), and the queue submits, resubmits on a moved head, or withdraws
only within that set. Removing the label from a previously synced pull request withdraws only
that label's own submission; a change submitted directly with `submit`, from another label, or
from `--file` is untouched. Priorities still come from `pN` / `priority-N` labels (default 100).
Draft pull requests are never ready, but the report names the drafts it skipped (`drafts`) so
they are not silently invisible.
For a shadow / soak kit — a `run --shadow` rehearsal or a `serve --shadow` daemon mirroring what
people marked ready — drive the queue with `sync --label ready` (or your ready label), not
`--all-open`, which deliberately lists every open pull request for measurements. Pair the label
sync with `serve --shadow` (or `run --shadow`) so validations are real but the real base never
moves; see the quickstart's shadow / soak kit.
With `require_approval_at_head` (off by default), the batched list call also carries reviews
(and is charged 3 requests per hundred results): only a pull request whose current head commit
is approved, by full commit id, in the latest verdict of a reviewer with write access (`OWNER`,
`MEMBER`, `COLLABORATOR`), with no such reviewer's latest verdict requesting changes, counts as
ready; missing review data never approves. Under the rule the author becomes the notification
target only when the change has no `--notify` subscription yet. The report's `unapproved` field names the rest; a queued one is withdrawn (its
submitter is notified through the withdrawn notification) instead of resubmitted, and a later
sync listing it with an approval at its new head re-queues it.
Priorities come from labels such as `p10` or `priority-10`, the landing planner's convention;
lower lands first, and the default is 100. A pull request carrying the priority label
(`mq-priority` unless `init --priority-label` names another) becomes a priority change; adding or
removing the label resubmits it (see "Priority changes"). When `--close-prs` is set, `sync` also retries closes
that were postponed. Exit status is 1 if any submission was refused.

Ready-file lines look like `PR [SHA] [head=REF] [priority=N] [after=N,N] [solo] [prioritized] [by=NAME] [notify=NAME]`, and
`#` starts a comment. Without `head=`, the head is `refs/pull/PR/head`.

### `merge-queue withdraw PR`

This command marks a queued change `withdrawn`. `--reason TEXT` (default `withdrawn by submitter`)
is recorded in the change's history. Submitting the change again puts it back in the queue. While
a run holds the writer lock, the withdrawal is recorded, and the run does not land a batch that
holds the change (see "Intake while a run holds the queue").

### `merge-queue retry PR...`

This command returns changes that were escalated to their owner (see "Bounded retry") to the
queue: it clears the escalation, the refusal count and the backoff, in the item records and in
the shadow ledger. Run it once whatever kept validation from running is fixed. A change that is
only deferred is returned at once too, without waiting for its backoff. It is also the ruling
that ends a hold (see "Held for a ruling"): the change is validated again. While a run holds the
writer lock, the retry is recorded and applied when the lock frees (see "Intake while a run holds
the queue").

### Intake while a run holds the queue

A run holds the writer lock from start to finish, which can be many minutes. `submit`,
`withdraw` and `retry` never wait for it and never fail because of it. When the lock is busy,
each one records its request as a file under `intake/pending/`, prints `recorded ...` and exits 0.
The next holder of the lock applies the recorded requests in the order they were recorded,
before it reads the queue. That holder is the `serve` daemon at its next poll, a `run`, a
`sync`, or the next `submit`, `withdraw` or `retry` that finds the lock free. A recorded request
does exactly what the command would have done then, and its history entry notes when it was
recorded.

- `submit` first checks, without writing to the repository the run is using, that the head
  resolves (`git ls-remote`), and that it matches `--sha` when one is given. A head that does
  not resolve is refused at once, as it would be with the lock free.
- `withdraw` and `retry` first check that the change is in the queue (and, for `retry`, queued).
  A withdrawal also takes effect inside the run that holds the lock: before landing a batch, the
  run reads the recorded withdrawals. If one names a change in the batch, the run does not land
  that batch, stops, and leaves the batch queued. The next cycle applies the withdrawal and
  validates the rest again.
- A request that cannot be applied when its turn comes (its change landed meanwhile, its head
  no longer resolves) is moved to `intake/refused/` with the reason, noted in the change's
  history (`intake-refused`), and shown by `status` for a week.

### `merge-queue list`

This command prints the queued changes as a table with the columns `PR`, `STATUS`, `PRIO`, `HEAD`,
`TESTED` and `TITLE`. `TESTED` shows the scope of the submitter's claim (`complete`, `focused`,
or `-` when there is none), so focused submissions are visible at a glance. `PRIO` ends in `!` for a
priority change.

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

The human form lists the batches with each change's priority and file count (and `priority,`
before them for a priority change), then the held changes, then the conflict clusters (connected
groups of conflicting changes with two or more members), then any changes for which the priority
label is suggested.

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
| `--offload MODE` | configured | Override `validator.offload` for this run: `off`, `compare` or `decide`. |
| `--json` | off | Print the report as JSON instead of Markdown. |

The exit status is `3` when the run stopped early; the report's `stopped` field says why. An
error in the middle of a run (a Git or validator failure) stops it the same way, and the report is
still written.

### `merge-queue serve`

This command runs the queue forever: deterministic code that is always running, so agents submit
changes and never drive the queue loop by hand. While idle it holds **no** writer lock. It polls
the `items/` directory fingerprint every `--poll-ms` (default `250`) and polls the remote base
with a cheap `git ls-remote` every `--base-poll-secs` (default `30`). A changed fingerprint (a
new `submit`, `sync` or `withdraw`, or a request recorded while a run held the lock and applied
at the top of the next poll) or a moved base triggers one work cycle: the daemon takes the
writer lock, plans, and runs the same `Runner` path as `run` (the logic is not forked), then
releases the lock and goes back to idle polling. Two triggers are time-based: a queued change
whose refusal deferral (`next_retry_at`) has passed is retried without any new submit, and a
failed cycle (an error, or a run that stopped early) is retried after 1 s, 2 s, 4 s, ... capped at
5 min, with the error kept as the heartbeat's `last_error` and the next retry time as its
`next_retry_at`. A cycle that finds the lock held or a failed base poll is recorded in the
heartbeat; none of these kills the daemon. While a cycle runs, a helper thread refreshes the
heartbeat every 0.5 s, so a long validation never reads as stale. An unreadable item record is
surfaced as `last_error` (the daemon plans nothing until it reads) rather than read as an empty
queue. One daemon per state directory is guaranteed by an exclusive `flock` on `serve.lock`,
held for the daemon's lifetime; `serve.pid` is informational, and a daemon removes it only while
it still holds its own pid. A graceful stop removes the pid file, a crash leaves it behind (and
frees the lock) for `status` to report.

**Stopping.** SIGINT or SIGTERM (Ctrl-C) stops the daemon at once, even mid-cycle, and a stop
is never a verdict. The run cancels its validations, which are recorded as cancelled. It starts
no new validation, landing or ejection, and exits with every change it had not decided still
queued (the run report's `stopped` names the stop). A regeneration or a landing already under
way finishes first; these take seconds. The same holds for a process the queue started that
dies of an outside kill signal (SIGTERM, SIGINT, SIGHUP or SIGKILL, from a service manager, the
OOM killer or an operator) while the daemon keeps running. A killed validation is no red
result and a killed generator is no `regen-failed`: the run stops with that reason, and the
next cycle retries. A crash signal such as SIGSEGV is still the process's own failure. A second
SIGINT or SIGTERM during a stop exits immediately. `merge-queue run` stops the same way.

| Option | Default | Meaning |
|---|---|---|
| `--poll-ms N` | `250` | Idle poll interval in milliseconds: `items/` watch and heartbeat cadence (at least 10). |
| `--base-poll-secs N` | `30` | Seconds between `git ls-remote` base polls. `0` polls the base every cycle. |
| `--once` | off | Run at most one work cycle, then exit. For scripts and tests. |
| `--shadow` | off | Validate for real, but land on a simulated base (the same shadow path as `run --shadow`): full plan/run/notify cycles execute, landings are recorded as shadow/simulated (`mode: shadow` in the run report, `mode: shadow` in `status`), item statuses stay queued, and the real base ref is never pushed. Daemon soak mode; also the remedy when `land.driver` is `none`. |
| `--depth N` | the configured `depth` | Override the initial window for work cycles. |
| `--max-batch N` | the configured `max_batch` | Override the batch size for work cycles. |
| `--agentctl WORD` | configured | Command (and leading arguments) to run instead of the configured agentctl for notifications. Repeatable. Supplement: `MERGE_QUEUE_AGENTCTL` (shell-style words) when neither flag nor config names one. |
| `--agentctl-registry PATH` | configured | agentctl registry the notifications address instead of the configured `notify.registry`. Supplement: `MERGE_QUEUE_AGENTCTL_REGISTRY` when this flag is not given. |
| `--notify-timeout-secs N` | `30` | Seconds one notification send may run; then its whole process group is killed and the send counts as a failed attempt (minimum 1). |
| `--sync-label LABEL` | none | Also ingest pull requests carrying LABEL, inside the daemon's own loop: the same ingestion as `merge-queue sync --label LABEL` (one batched list call, approval at head when configured, withdrawal when the label goes away), under the writer lock the work cycle uses. Needs `github.repo`. |
| `--sync-every SECS` | `60` | Seconds between label syncs (with `--sync-label`; minimum 10). Each sync spends at least one hosting API request. |
| `--offload MODE` | configured | Override `validator.offload` for this run: `off`, `compare` or `decide`. |

With `--sync-label LABEL` the daemon ingests labelled pull requests itself, so a change
labelled ready that nobody submits is never missed. Each sync runs at the top of a loop
iteration, before the daemon looks for new work, under the same writer lock the work cycle
takes (never a second writer). What it submits is therefore validated in the same iteration.
A busy lock (a `run` or another command holding it) retries on the next poll. A failed sync
(the hosting service down, the request budget spent) is logged once per distinct error and
recorded as `sync_error` in the heartbeat and `status`. It never stops validation of what is
already queued. A successful sync records `last_sync_at` and a one-line summary.
Notifications its withdrawals owe go out through the daemon's own `--agentctl` and send
timeout.

With `--shadow` the daemon is a soak harness: it plans, validates and would land exactly as a
live daemon would, but every landing advances only the in-memory simulated base. Run reports
carry `mode: shadow`, the heartbeat carries `shadow: true`, and `merge-queue status` shows
`mode: shadow (landings simulated; real base unchanged)` so a shadow daemon is never mistaken
for a live one. Because no item really lands, no `landed` notification is owed in shadow mode;
the notification sweep still runs each cycle for the transitions that are real (withdrawals,
refusals). A queue whose `land.driver` is `none` refuses live cycles with a remedy naming
`--shadow`; `serve --shadow` is that remedy.

## Notifications

A change submitted with `--notify NAME` (or a ready-file `notify=NAME`) gets exactly one push message per state change — `landed`, `ejected`, `refusal-deferred`, `withdrawn` — from the `serve` daemon:

```text
agentctl send --deliver-while-working --file <state>/notifications/<report>.md -- NAME
```

(`--` ends the options, so a name starting with `-` is never read as one.)

The report names the change, the exact candidate (submitted head) SHA, the verdict, the landed base commit (or the failing tests / refusal detail and retry time), and the run-report path. Delivery state (transition key, attempts, delivered flag, last error) is persisted on the item record as `notify_delivery`, so daemon restarts and extra cycles never resend a delivered transition. A failed send — including a missing agentctl binary (126/127 are failures, never success) — is logged to the daemon's stderr and the item's history, retried on later cycles at most 3 times in total, and never blocks or fails the queue. Each send runs in its own process group with a deadline (30 s, `serve --notify-timeout-secs`); a send still running then is killed with its group and counts as a failed attempt. The sweep runs only under the writer lock (the daemon skips a cycle's sweep when the lock is busy; `sync` runs it under the lock it already holds), and records delivery on a fresh read of the item, changing only `notify_delivery` and the history, so it never overwrites another writer's change. The command is configured with `init --agentctl WORD...` (stored as `notify.agentctl`), overridden per daemon with `serve --agentctl WORD...`, and supplemented by `MERGE_QUEUE_AGENTCTL`. The registry the sends address resolves the same way, from `serve --agentctl-registry PATH`, then `MERGE_QUEUE_AGENTCTL_REGISTRY`, then `notify.registry` (`init --agentctl-registry PATH`); when one is set, each send runs `<command> --registry PATH send ...`. Set one whenever the daemon does not start in a directory holding the registry: agentctl's default is `.agentctl` in its working directory, and a daemon started in a bare queue repository has none there, so every send fails with `cannot inspect agent registry <repo>/.agentctl`.

### `merge-queue status`

This command reports daemon liveness, the current window and slot occupancy without taking the
writer lock. Liveness comes from `serve.lock`, `serve.pid` and `serve.heartbeat.json` in the
state directory (the daemon rewrites the heartbeat every cycle, and every 0.5 s during a work
cycle): `running` (pid alive, heartbeat fresh), `not running` (no pid file), `dead` (pid file
names a dead process, or `serve.lock` is free so the pid now belongs to something else), `stale`
(pid alive but the heartbeat is older than `--stale-after-secs`), or `malformed` (pid file
unreadable). A daemon in a work cycle is reported as `working for N s` (JSON `working_secs`)
separately from liveness; with a fresh heartbeat it is `running` and no kill is advised. An
unreadable item record makes `status` unhealthy (JSON `items_error`, exit 1) instead of
counting zero queued changes. Every unhealthy verdict names its remedy. The window is the
batches a live daemon's heartbeat names: none while it idles, and none when no daemon is
running (a `run` started by hand reports its own progress). Slots count the `scratch/slot-*`
checkouts; one is in use only while a validation holds its `scratch/slot-<n>.lock`, since every
slot keeps its checkout between validations. A failed attempt to take the writer lock shows as
the daemon's last error until the daemon gets the lock or has nothing queued. Requests recorded
while a run held the lock show as `intake: N request(s) ...` until they are applied. Requests
refused when applied show as `intake refused: ...`, and the newest three are named (JSON `intake`).

| Option | Default | Meaning |
|---|---|---|
| `--json` | off | Print the status report as JSON (`daemon`, `pid`, `heartbeat`, queue counts, `active_batches`, slots, `intake`, `remedy`). |
| `--stale-after-secs N` | `10` | Heartbeat age in seconds after which a live daemon counts as stale. |

### Running `serve` as a service

A systemd user unit template ships at `rs/merge-queue/merge-queue-serve.service`
(`Restart=on-failure`, `ExecStart` pointing at `serve`). Install it with
`merge-queue install-serve-unit`, not by copying the template by hand: a systemd user
service does not inherit a login shell's `PATH`, so the installer resolves the toolchain
directory at install time (from `rustup which cargo`, then `cargo` on `PATH`, then
`$CARGO_HOME/bin` or `~/.cargo/bin`; override with `--toolchain-bin DIR`) and writes it
as an absolute `Environment=PATH=...` in the rendered unit. At startup `serve` also
refuses, naming the missing tool (for example `cargo not found`) and the `PATH` it searched,
when any executable its configured regenerate/validator commands name does not resolve. Relative
paths such as `common/bin/dagrun` name files inside the candidate checkout, which exist only at
run time, so they are not checked. Tools such a wrapper calls are not checked either. A tool
missing at run time is an infrastructure refusal (see **A validator that cannot execute** under
Validators), never a red verdict. Set an explicit `Environment=PATH=...` in any unit you write
yourself, naming every directory the validator and the tools it calls live in: a service's
default `PATH` is much shorter than a login shell's.

The unit sets `KillMode=mixed`, so `systemctl stop` (or a restart) signals only the daemon. The
daemon then cancels its own validations and records them as cancelled before it exits (see
**Stopping** above). Whatever is still running when `TimeoutStopSec` runs out is killed. Keep
`KillMode=mixed` in a unit you write yourself. Under the default control-group mode every
validation is signalled with the daemon. The queue reads that death as no verdict too, so
nothing is ejected, but the validation is not recorded as cancelled.

```bash
merge-queue install-serve-unit   # or: --print to preview, --output FILE to relocate
systemctl --user daemon-reload
systemctl --user enable --now merge-queue-serve
merge-queue status   # daemon: running ...
```

The tick-hub launcher alternative ships at `rs/merge-queue/tick-hub-merge-queue.yaml`: each tick
runs `merge-queue status` as a gate and emits an action asking for a restart when the daemon is
not healthy, plus a heartbeat-freshness health check. Edit its paths, then
`tick-hub tick --config rs/merge-queue/tick-hub-merge-queue.yaml`.

### `merge-queue donor`

Donor validators (see "Donor validators"). `donor run` is the donor daemon, run on any host with
a clone of the queue's repository. The other verbs are what a donor calls on the queue host
through its route; an operator uses only `status`.

| Verb | What it does |
|---|---|
| `run --queue CMD --clone DIR --name NAME [--jobs N]` | Claim jobs through the route, validate each exact candidate in a worktree of `DIR`, report receipts. Options: `--remote` (default `origin`; fetched for the base), `--poll-secs` (default 5), `--work DIR` (default `merge-queue-donor/` in the clone's Git directory), `--mismatch-backoff-secs` (default 300), `--until-idle`. SIGINT/SIGTERM gives running jobs back and exits. |
| `status [--json]` | Jobs by state, leases (never their tokens), and in `compare` mode how many receipts matched the local verdict. |
| `claim --donor NAME [--host H] [--jobs N]` | Lease the oldest job this donor may take; prints `{"job": ...}` (with the token) or `{"job": null}`. |
| `renew --job ID` | Token on stdin. Prints `{"renewal": "continue", ...}` or `{"renewal": "stop", "reason": ...}`. |
| `bundle --job ID` | Token on stdin. Writes the candidate's git bundle to stdout. |
| `report --job ID` | `{"token", "receipt"}` on stdin. Exits 1 with the reason when the receipt is refused. |
| `release --job ID [--reason R] [--decline]` | Token on stdin. Gives the job back; `--decline` means this donor does not get it again unless the job is re-stamped. |

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

### `merge-queue stats`

This command summarises the decision log (see "The decision log") for a window: the batch-width
distribution (average, minimum, maximum and a count per width), the success rate per width
(green over green plus red; refused and cancelled batches have no verdict), wall and CPU
validate-hours, changes landed per validate-hour, culprits named, and ready-to-land latency
percentiles (nearest rank). Shadow runs count too, with their simulated landings; `--mode`
separates them.

| Option | Default | Meaning |
|---|---|---|
| `--since TIME` | `24h` | Window start (inclusive): an RFC 3339 time, or a duration before now (`30s`, `90m`, `24h`, `7d`). A record belongs to the window its validation ended in. |
| `--until TIME` | now | Window end (exclusive), in the same forms. |
| `--mode MODE` | every mode | Count only `run` (real landings), `shadow` (simulated landings) or `widen` validations. |
| `--reconstruct` | off | Rebuild the records from the run reports under `runs/` instead of reading the log, for windows before the log existed. The output lists what cannot be reconstructed (below). |
| `--records` | off | Print the window's records as JSON lines instead of the metrics. |
| `--json` | off | Print the metrics as JSON. |

`--reconstruct` reads each run's `report.json` (and, for a run killed before it wrote one, its
`validations/*/record.json`, which give no landings or ejections) and the item histories. It
recovers every validation's changes, commit, purpose, width, wall time, selection, outcome and
failed tests; what it landed, and when (from the landing event in the item history); each
change's ready time (its last submission before the validation ended); the culprit of a red batch
(the one failed-tests ejection among its changes); the donor of a donor validation (from the
first line of its log, while the log exists); and the features below (sizes need the commits to
still be in the queue's repository). It cannot recover the CPU time, the speculation depth and
window per batch, or the base a candidate was combined onto, which no earlier report recorded,
nor a predicted pass probability, which nothing computes yet.

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
4. **Pairwise conflicts.** Pairs of queued heads are merged with `git merge-tree`. Two kinds of
   pair are skipped because they cannot conflict: stacked pairs, found from one
   `git rev-list BASE..HEAD` per head rather than a check per pair, and pairs that share no file
   outside the generated files (sharing a file is a necessary condition for a textual conflict).
   The merges run on long-lived `git merge-tree --stdin` processes, one per worker
   (`probe_parallelism` in `config.json`). Answers are cached in `probe-cache.json`, keyed by exact commits and a
   hash of everything else that can change a merge: the queue's union and regeneration settings,
   the repository's and the global attributes files, `HEAD:.gitattributes`, merge configuration
   and the Git version. The cache is pruned to the current base and queued heads on every plan.
   On a fixture of 95 changes over 3,000 files, each also rewriting one generated file, planning
   takes about 1.6 s cold (about 4 CPU-seconds) and 1.35 s warm, against 11 s cold and about
   265 CPU-seconds with one merge process per pair.
5. **A conflict-free subset.** The queue does no repair. Once one of two conflicting changes
   lands, the other conflicts with the base, so both cannot land in the same round. The queue
   walks the changes in dependency order and picks the best-ranked ready change each time. The
   rank is: priority changes, and the changes they must land after, first; then lower priority
   value; then fewer conflicts with other eligible changes; then
   earlier submission; then lower number. A change that conflicts with one already chosen is held
   as `conflicts-with-queued`. Preferring fewer conflicts is a greedy maximum-independent-set
   heuristic. When one change conflicts with many independent ones, it lands more changes per
   round than first-come-first-served.
6. **Batches.** The chosen order is cut into batches of at most `max_batch` changes. A `--solo`
   change, an exact change and a priority change each close the open batch and get a batch of
   their own. Each batch has a `note` saying why it was cut: `max-batch`, `solo`, `exact`,
   `priority`, or empty for the last one.
7. **Label suggestions.** A queued change that is not a priority change and touches only the
   configured CI paths (`priority.ci_paths`) is listed in `suggest_priority`. It is a suggestion
   only; the queue never applies a label.

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
             "files": 3, "conflict_degree": 1, "prioritized": true}],
  "batches": [{"index": 0, "prs": [42, 43], "note": "max-batch"}],
  "held": [{"pr": 41, "kind": "conflicts-with-queued", "reason": "...",
            "with": [42], "paths": ["src/cli.rs"]}],
  "conflicts": [{"a": 41, "b": 42, "paths": ["src/cli.rs"]}],
  "dependencies": [[42, 43]],
  "suggest_priority": [44]
}
```

- `items` lists every queued change that was considered. `head` is empty when the head ref did not
  resolve. `conflict_degree` counts conflicts with eligible changes.
- `batches` are in landing order. `index` starts at zero. `prs` are in combine order. `note` is
  left out when empty.
- In `held`, `with` and `paths` are left out when empty.
- `dependencies` holds `[before, after]` pairs that the plan respected.
- `prioritized` is present (and `true`) only for a priority change. `suggest_priority` lists the
  changes the priority label is suggested for, and is left out when empty.

## State directory

```text
<state>/
  config.json            fixed: queue configuration
  lock                   fixed: writer lock (flock)
  serve.lock             fixed: single-daemon lock (flock held by a running `serve`)
  serve.pid              fixed: `serve` daemon pid (informational; removed on a graceful stop)
  serve.heartbeat.json   fixed: `serve` heartbeat, rewritten every cycle
  union.attributes       fixed: generated from union_paths, when set
  items/<pr>.json        one record per submitted change
  notifications/        notification report files the daemon sends with `--file`
  runs/<run-id>/         one directory per run (layout below)
  decisions/<day>.jsonl  the decision log: one record per validation, by UTC day (see "The decision log")
  obligations/<id>.json  one record per focused landing still owed a complete run
  scratch/slot-<n>/      one reused worktree per concurrent validation
  scratch/slot-<n>.lock  flock held by the run while a validation uses slot <n>
  intake/pending/<id>.json   one request recorded while a run held the writer lock, until applied
  intake/refused/<id>.json   one recorded request that could not be applied, with the reason (kept a week)
  donor/lock             fixed: donor job store lock (held for milliseconds)
  donor/jobs/<id>.json   one record per donor job (mode 0600: it holds the lease token)
  donor/scratch/         bundles being sent to donors (removed after each)
```

A run directory holds `plan.json` (the plan it started from), `events.jsonl` (every step, in
order), `validations/<NNNN-purpose>/` (one per validation: `run.log`, the runner's results and
`record.json`), `ejections/<pr>.md` (one report per ejected change), `report.md` and
`report.json`. The scratch slots are kept between validations and runs: switching a slot to a new
candidate is a forced checkout plus `git clean -ffd`, which keeps ignored build caches, so later
builds are incremental (`validator.clean_ignored` adds `-x`). There is one slot per possible
concurrent validation, plus one for selection queries.

Growing collections live in their own subdirectories. Records are written atomically (temporary
file, then rename). `run`, `sync` and `widen` take the writer lock and fail fast if another writer
holds it. `submit`, `withdraw` and `retry` record their request when it is held (see "Intake
while a run holds the queue").

An **item record** (`items/<pr>.json`) has these fields:

- `pr`, `head_ref`, `head`, `priority`, `submitted_at`, `submitted_by`, `notify`, `notify_delivery`, `title`, `after`, `solo`, `prioritized` (present only when `true`).
- `claim`: `{selection, policy, scope, commit}`. `selection` is `{"kind":"all"}` or
  `{"kind":"tests","tests":["step","step::case"]}`.
- `status`: `queued`, `landed`, `ejected` or `withdrawn`.
- `landed_as`: the base commit that contains the change, once it has landed.
- `validation_refusals`, `next_retry_at`, `last_refusal`: present only after a validation
  refusal. The status stays `queued`; `next_retry_at` is when validation may be retried.
- `flaky_nodes`, `flaky_hold_until`, `hold_verdict`: present only while the change is held for a
  ruling (see "Held for a ruling"). `hold_verdict` is `INFRA-BLOCKED` for that hold and absent
  for a GREEN_FLAKY one.
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
| `priority` | absent | `{label, ci_paths}`: the priority label (absent or empty: `mq-priority`; `-`: off) and the CI path patterns for label suggestions. Left out of the file while both are defaults. See "Priority changes". |
| `regenerate` | empty | `{paths, command, when}`; see "Generated files". |
| `exact_sha` | `false` | The exact-commit rule. |
| `trust_base` | `false` | Skip probing the base in culprit searches. |
| `require_approval_at_head` | `false` | Approval-at-head rule for GitHub ready sources; see `--require-approval-at-head`. |
| `recount_pins` | `false` | Recount the generated count pins after merging each batch (base plus each change's own delta); see `--recount-pins`. A change whose pins cannot be recounted is skipped from its batch and stays queued. |
| `pins` | `[]` | `[{name, gitlink, files, url, branch}]` commits of other repositories this one pins, resolved to the descendant when changes are combined; see "Pinned commits of other repositories". |
| `classify` | `[]` | `[{pattern, class, evidence}]` failure classes (`evidence` left out of the file while empty); see `--classify`. |
| `peers` | `{}` | Peer queue name to state directory. |
| `identity` | empty | `{name, email}` for commits the queue creates. |
| `github` | empty | `{repo, gh, max_per_hour, budget_file, anonymous_fetch}`; see "Ready sources and the request budget". |
| `validator` | empty | How validation runs; see below. |
| `land` | `push` from `init` | How a validated candidate reaches the base; see below. |
| `notify` | empty | `{agentctl, registry}`: command words for state-change notifications, and the agentctl registry they address (left out of the file while empty); see "Notifications". |
| `classifier` | absent | `{policy}`: the test-footprint policy path for decision-log features (see `--classifier-policy`). Left out of the file while unset. |
| `decisions` | absent | `{keep_days, max_bytes}`: bounds of the decision log, pruned after every run (absent or `0`: 90 days and 64 MiB; the newest day file is always kept). Left out of the file while both are defaults. |

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
- `offload`: `off` (or absent; the default), `compare` or `decide` (see "Donor validators").
- `tool_identity`: `{name: command}`, the tools a validation depends on, checked on donors.
- `tool_refresh`: `{name: command}`, how a donor brings each of those tools up to date when its
  answer differs from a job's (see "Tool identity and receipts").
- `max_deferrals`: deferrals before a change is escalated to its owner (absent or `0`: `3`).
- `red_without_probe`: `hold` holds a red change whose probe cannot run for its owner; absent
  (the default) ejects it.

**`donors`** fields (only read when `validator.offload` is not off; absent means the defaults):

- `lease_secs`: how long a lease lasts without a renewal (default `120`; a donor renews every
  quarter of it).
- `max_bundle_bytes`: the largest candidate bundle served to a donor (default 256 MiB).
- `max_jobs_per_donor`: the most leases one donor may hold (default `0`: what the donor asks for).
- `claim_timeout_secs`: in `decide` mode, how long a job may wait with no donor holding it
  before the run stops with an error (default `900`). Nothing is ejected.
- `max_could_not_run`: could-not-run receipts one job may collect before it is cancelled, and
  one donor may return for it before it is left out (absent or `0`: `3`).

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
runner exits `75`, or it exits non-zero, its first non-empty output line starts with
`COULD_NOT_RUN`, and it attributed no failed test — did not run, so it is
neither a pass nor a failure. The queue records the validation as `refused` with no failed
tests, retries it in the same run after a backoff (`refusal_backoff_secs` doubled per attempt,
up to `refusal_max_retries` extra attempts), and if it is still refused, defers the batch: every
change in it stays `queued`, its `validation_refusals` count grows, `next_retry_at` is set from
the backoff, the run report lists it under `deferred`, and the run stops with exit `3`. No
culprit search runs and nothing is ejected for a refusal. While `next_retry_at` is in the
future, `plan` holds the change as `validation-refused`; after it passes, the next `run`
validates it again. A refusal in a culprit-search probe or a widening run defers the same way
(no culprit is named, no obligation is discharged or marked red).

**A validator that cannot execute is a refusal too.** When the runner (or the admission prefix,
or the dagrun launcher) cannot be started, or it exits `126` (a command was found but could not
run) or `127` (a command was not found, which is the shell's answer when the validator or a tool
it calls is missing from `PATH`) without reporting a failed test itself, validation never ran.
The host is at fault, not the change, so this is handled exactly like an admission refusal:
there is no verdict, the change stays queued, and the validation is retried with backoff. The
refusal reason names what happened, the command, the `PATH` it ran with and the runner's last
output line, for example:

```text
the validator could not execute (exit 127: a command was not found): command sh -c 'sh ./validate.sh'; PATH=/usr/bin:/bin; last output: ./validate.sh: line 1: mytool: command not found
```

The reason goes into the item's `last_refusal` and history, the run report's `deferred` entry,
and the `refusal-deferred` notification. `serve` also prints it as an `ALARM:` line and keeps it
in the heartbeat's `last_error` until a clean cycle. A runner that reports failed tests and then
exits `127` is still red: its own results are evidence.

**Bounded retry.** Validation that did not run is retried, but never forever. Each deferral of
a change counts, whatever caused it: an admission refusal, a validator that cannot execute, a
culprit-search probe that could not run on an unattributed red, or donors that could not run.
When a change reaches `validator.max_deferrals` (default `3`), it is escalated to its owner
instead of deferred again. The change stays `queued` with an `escalated` reason, `plan` holds
it as `escalated`, and no run builds it into a candidate. Its `--notify` recipient gets an
`escalated` notification, and the run and `serve` print an `ALARM:` line (also kept in the
heartbeat's `last_error`). `merge-queue retry PR` returns it once the cause is fixed. A new head
starts the count again. Shadow runs write no item records, so they count in
`<state>/shadow-refusals.json` and hold escalated changes the same way.

**A red run is a verdict even when its probe cannot run.** After a red validation the queue
probes the failed tests on prefixes of the batch, to name the culprit change and to tell a flaky
test from a real failure. When such a probe cannot run (refused, or the validator cannot
execute) and the red run named failed tests, the red run still decides. A batch of one change
is ejected, with a reason saying that culprit narrowing and the flaky check were skipped, or
held for its owner under `validator.red_without_probe: hold`. A larger batch cannot be split
without probes, so each of its changes goes back as a batch of its own, validated on its own
commit. A red run with no attributed failures is deferred within the bound. The red commit
itself is never validated again.

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

**Held for a ruling.** Two marker lines let a runner say that its run is neither a plain pass nor
a product failure; each names the tests it concerns:

- `GREEN_FLAKY: id1,id2` with exit `0`: the run is green only because those tests passed after a
  retry, inside the run or because the run was itself a same-commit rerun of a failure.
- `INFRA-BLOCKED: id1,id2` with exit `6`: the host refused every measurement of those tests, on a
  run and on its rerun of the same commit, so the commit has no result for them.

The queue holds such a change instead of landing or ejecting it: the batch run that printed the
marker, or a culprit search whose rerun of the whole candidate printed it. A marker with another
exit status means nothing. Each change of the batch stays `queued` with the tests in
`flaky_nodes`, the hold's end in `flaky_hold_until` (`--green-flaky-hold-secs`, two hours by
default) and, for INFRA-BLOCKED, `hold_verdict: INFRA-BLOCKED`. `plan` holds it as
`held-green-flaky` or `held-infra-blocked`, its `--notify` recipient gets that notification, and
batches built on top of it are discarded. `merge-queue retry PR` is the ruling that returns it to
the queue; when the hold ends without one, the change is re-queued for a fresh validation (which
holds it again if the result is the same). A donor's receipt carries the markers in its log tail.

**`dagrun`.** Test IDs are DAG step names. A selection becomes `dagrun run --dag DAG --keep-going
--selected a,b`, which also runs each selected step's dependencies; `all` omits `--selected`. The
per-step verdicts come from the run journal (`journal.jsonl` under a private `DAGRUN_LOG_DIR`):
`step_end` gives pass or fail with the wall time, `step_skip` gives skip. A case-level ID narrows
nothing for this kind; its whole step runs.

## Donor validators

The queue validates every candidate on its own host, which caps throughput at that host's few
concurrent validations while other hosts sit idle. Donor validators let other hosts take queued
validations, run them with the same validator, and return receipts for the exact candidate.
Landing never moves: only the queue host writes to the base, fast-forward to the next green
candidate, exactly as without donors.

**Strictly opt-in.** `validator.offload` is `off` unless an operator sets it, and `off` is
exactly the in-process path. The two other modes:

- `compare`: the queue validates in process as always, and that verdict alone decides. Every
  validation is also published for donors. When a donor's receipt arrives, it is compared with
  the local verdict, and `donor status` counts how many matched. Run this first: a donor whose
  environment differs subtly shows up as a mismatch and never lands anything. Switch to
  `decide` only after a run of matching verdicts.
- `decide`: validations run on donors only, and an accepted receipt is the verdict, with the
  same culprit search, flaky detection and obligations as a local result. The queue host keeps
  validating by running a donor of its own (`donor run --queue 'merge-queue --repo QUEUE'
  --clone QUEUE --name local --jobs 2`), so its local capacity is one donor among several.
  Remote donors add capacity.

**Donor setup: the same repository.** `--clone` must be a clone of the queue's repository.
Before it claims anything, `donor run` asks the queue host for its repository's remote URL
(`donor info`) and compares it with the URL of its own `--remote`. A different repository
refuses to start. The same repository in another URL form is accepted with a note: `https://`,
`ssh://`, scp-like `git@host:path`, with or without `.git`, a trailing `/`, a user or a port.
`donor info` also reports the queue host's merge-queue revision, and a donor whose revision
differs warns at startup. A validator that checks its checkout's origin itself should compare
the same way. `merge-queue donor same-repository URL URL` exits `0` when two URLs name one
repository and prints both normalized forms.

**Transport: no new ports.** A donor reaches the queue only by running this tool on the queue
host through its `--queue` route. Each request appends its arguments to the route, for example
`agentcloudctl node exec --node QUEUEHOST -- /path/merge-queue --repo /path/queue`,
`ssh QUEUEHOST merge-queue --repo ...`, or a local path. The route is the authentication: only
someone who can already run commands on the queue host as its owner can change the store.
Requests are short calls that print JSON. The lease token travels on stdin, never in arguments,
so it never appears in process listings, logs or `status`.

**Leases.** Each validation becomes a job in `<state>/donor/jobs/`, behind its own lock: donors
never wait for the writer lock a running work cycle holds. A claim leases the oldest job the
donor has not declined (see "Tool identity and receipts") to exactly one donor, with a fresh
random token.
- The donor renews every quarter of `donors.lease_secs`. A renewal answers "stop" when the job
  was cancelled (its candidate was discarded, or the queue is stopping) or the lease is no
  longer the donor's, and the donor then kills its run.
- A donor that dies stops renewing, its lease expires, and the job goes back to the queue for
  the next claimant. A job never runs twice at once, and a late receipt from the old holder is
  refused.
- `donor run --jobs N` bounds a donor's concurrent jobs, and `donors.max_jobs_per_donor` caps
  every donor on the queue side.

**The candidate tree.** Candidates (merges, regenerations) exist only in the queue's
repository. A donor whose clone already has the commit uses it (the queue host's own donor
always does). Otherwise the donor fetches `--remote` so it has the job's base, then asks for a
bundle of the candidate over the base. It checks that the commit and tree are exactly the job's
before running anything. A bundle over `donors.max_bundle_bytes` (default 256 MiB) is refused,
and the donor declines the job, leaving it to a donor that has the commits. A route carries
about 25 MB/s, so the default takes about ten seconds. A candidate that adds more than that
over the base is normally a sign of a stale base.

**Tool identity and receipts.** A job carries the validator settings, the selection (decided on
the queue host) and the tool identity, which has three parts:
- a digest of those settings;
- the merge-queue revision of the queue host's binary;
- the answers of the configured `validator.tool_identity` commands, for example the commit of
  each tool repository and the pinned toolchain.

The commands run in the queue's repository on the queue host and in the donor's clone on the
donor. A donor that does not match the job names each difference, backs off claiming for
`--mismatch-backoff-secs`, and does not run the job. It declines the job only when it is
certain it can never match it. Otherwise it gives the job back and is offered it again:
- **A different merge-queue revision**: only an upgrade can match it, so the donor gives the job
  back. Once upgraded and restarted, it claims at once and takes the job.
- **A different tool answer**: the donor runs the tool's `validator.tool_refresh` command (for
  example a fetch of the tool repository; `init --tool-refresh NAME=COMMAND`) once and asks
  again. A donor that was older than the job now matches, and runs it. A donor that still
  differs is newer than the job, or unrelated to it, and declines. It is offered the job again
  only when the job is re-stamped, after the queue host's tools move. A tool with no refresh
  command, or whose refresh fails, cannot be put in order, so the donor gives the job back
  instead of declining it. Refreshes run only on a mismatch, never on a timer, and each is
  bounded at 120 s.

A queued job is re-stamped with the queue host's current answers at every claim (when the
queue host can answer them), so every queued job carries the same stamp. A decline with the reason a donor built before this rule
gave for a mismatch (`this donor's tools differ from the queue host's: ...`) is recorded as a
release.

Identify a tool by the revision it was built from, never by a hash of its binary: two builds of
one revision on different hosts are not byte-identical, so binary hashes never match. merge-queue
records its own revision at build time and prints it as the second line of `--version`
(`commit <sha>`, with ` (dirty)` after it when the build had uncommitted changes):
- A build inside a Git checkout records that checkout's `HEAD`. It is dirty when this crate, its
  path dependency, or the workspace manifest or lock file has uncommitted or untracked changes.
- A build outside a checkout (from an archive) records nothing unless the builder sets
  `MERGE_QUEUE_SOURCE_COMMIT` (the full commit) and optionally `MERGE_QUEUE_SOURCE_DIRTY` (`0` or
  `1`).

A binary that recorded no revision, or a dirty one, cannot be matched. A queue host with such a
binary publishes nothing: `compare` mode runs with donors off and a warning, and `decide` mode
refuses to start. A donor with one refuses to start. For development and tests,
`MERGE_QUEUE_ALLOW_DIRTY=1` on every host accepts dirty builds, compared by commit only.

The receipt (`merge-queue-receipt/v1`) names:
- the job, the donor and its host, the commit and tree it validated, and the tool identity it
  checked;
- its merge-queue version, the exit status, whether admission refused, the per-test results,
  the timing, the end of the log, and where the full log stays on the donor host.

The queue accepts it only if all of these hold:
- the lease is live and the token and donor name match;
- the commit and tree are the job's;
- the tool identity equals the one the queue computed when it published the job.

Every refusal is recorded in the job's history. Validation timeouts are the validator's own
(`validator.timeout_secs`), enforced on the donor exactly as locally.

**Validation that never ran on a donor.** A donor that cannot execute the validator (see
**A validator that cannot execute**) declines the job with that reason and backs off, and it
sends no receipt. A receipt that says validation could not run (admission refused, exit `75` or
`COULD_NOT_RUN`) is no verdict in either mode. The job goes back to the queue, and the attempt
is recorded in the job's `could_not_run` list. `donor status` shows "could not run: N
receipt(s)", and compare mode compares nothing for it. In `decide` mode the waiting run prints an
`ALARM:` line naming the donor. A donor that answers this way `donors.max_could_not_run` times
(default `3`) for one job is left out of that job. A job that collects that many such receipts
is cancelled. A `decide` run waiting on it then counts the batch as refused, and the deferral
bound applies (see "Bounded retry").

**Stops, discards and dead candidates.** A stop request or a discarded speculative candidate
cancels the job. So does a candidate the queue no longer wants:
- When a batch lands, every job whose candidate is now in the base is cancelled ("its candidate
  landed").
- When a run ends, every job it published that is still queued or leased is cancelled as
  superseded: the next run plans its candidates afresh and publishes new jobs.
- A run starts by cancelling any job an earlier run left live because it was killed before it
  finished.

A donor holding a cancelled job is told to stop on its next lease renewal, and a receipt for the
job is refused. In `compare` mode only unleased jobs are cancelled this way. A leased job is
kept for its donor, because its receipt cannot affect landing and the comparison is still worth
having. It is compared when it arrives, and `donor status` counts it under "compared after
landing", which shows when donors are slower than the queue host. If that lease ends without a
receipt (it expires, or the donor gives the job back), the job is cancelled rather than queued
again. In `decide` mode a job no donor takes within `donors.claim_timeout_secs` stops the run
with an error that asks whether a donor is running and whether its tools match. Nothing is
ejected either way.

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
   with the tests recorded under `flaky` in the report, unless the runner marks that rerun
   `GREEN_FLAKY` or `INFRA-BLOCKED`, which holds the batch for a ruling instead (see "Held for a
   ruling"). Flaky results are never hidden.
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

## The decision log

With fewer validation slots than candidates, every run chooses which speculative validations to
run. Each validation leaves one JSON record in `decisions/<YYYY-MM-DD>.jsonl` (the UTC day it
ended), written when the run finishes, so those choices can be measured and the metrics they move
optimised. `merge-queue stats` reads it. Its fields (schema `merge-queue-decision/v1`):

- `at`, `started_at`: when the validation ended and started. `run`, `mode` (`run`, `shadow` or
  `widen`), `validation` (its number in the run) and `purpose` (`batch`, `probe`, `remainder`,
  `widen`).
- `changes`: the candidate's changes in combine order, each with its `head`, `ready_at` (its last
  submission before the validation ended), `ready_to_land_s` (only on the validation that landed
  it) and `features`.
- `commit`, `base` (the commit the changes were combined onto) and `width` (number of changes).
- `speculation`: for a batch, `{depth, window}`: 1 for a batch on the base, 2 for one on top of an
  unvalidated batch, and the window when it started.
- `predicted_pass`: the predicted probability of a green result; `null` until batching priors
  exist.
- `validator`: `{kind, host}`, `local` or `donor` with the donor's `NAME@HOST`.
- `cost`: `{wall_s, cpu_s}`. CPU time is the runner's and its waited-for descendants', read from
  `/proc` as the runner exits; `null` where it cannot be read (a killed runner, a system without
  `/proc`).
- `selection` (`all` or `N tests`), `outcome` (`green`, `red`, `refused` or `cancelled`),
  `failed` (the failed tests of a red validation), `culprit` (the change a red batch's or
  remainder's search ejected) and `landed` (`{at, prs}` on the validation that landed changes).

Each change's `features`, for the log and for future batching priors:

- `lines_added`, `lines_deleted`, `files`: its size against the plan's base (`null` when Git no
  longer has its commits).
- `components`: with `classifier.policy` set, the components the shared change classifier
  names (its footprints' components and `backend:ID` per backend rule); otherwise the first
  directory of each changed path (`(root)` for a file at the top).
- `classification`: with `classifier.policy` set and readable at the plan's base, `{verdict,
  unknown, only_force_full, only_inert}`: the policy's `skip`, `selective` or `full` for the
  change, how many paths matched no rule, and whether every path forces the full suite or is
  inert.
- `conflict_degree` and `priority` as the plan saw them, and `prioritized`.
- `trigger`: how it entered the queue, `submit` or the `sync` source (`label:NAME`, `file:PATH`).
- `ci_only`: every changed path is a configured CI path (`priority.ci_paths`).
- `claim`: soft-green evidence, the submitter's own validation claim: `{scope, tests, policy,
  at_head}`, `at_head` saying the claim names the submitted head (or no commit).

The log is pruned after every run to `decisions.keep_days` (default 90) and
`decisions.max_bytes` (default 64 MiB), oldest day first; the newest day is always kept. Writing it
never fails a run: a failure is recorded as a `decisions` event.

## Priority changes

A change that repairs the CI or validation infrastructure unblocks everything behind it, so it
should not wait its turn. Mark it as a priority change: give the pull request the priority label
(`mq-priority` by default; `init --priority-label` names another, and `sync` matches it without
regard to case), submit it with `--prioritized`, or add `prioritized` to its ready-file line.

- **Planned first.** A priority change, and any change it must land after, ranks ahead of all
  other work, whatever their numeric priorities. It also wins a conflict with an ordinary change.
- **Validated alone.** It gets a batch of its own, so it is validated directly on the base. A
  red result names only it: it cannot hide behind unrelated changes, and it cannot get them
  ejected. Batches speculatively stacked on it are rebuilt, as behind any red batch.
- **Landed only on its own green.** Nothing is skipped: it lands only when its own exact commit
  validates green, like every other change.

With `init --ci-path PATTERN` (repeatable; `priority.ci_paths`), `plan` names the queued changes
that touch only those paths as candidates for the label. The queue never applies the label: a
person or agent decides.

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

A label read from the test's name alone also lands on every other failure of that test: a real
test failure in a counted suite would be called pin drift, and its author sent to fix the wrong
thing. Give such a class its evidence, the text the failure itself prints
(`--classify 'test.*_count=count pin drift=expected * tests to execute, saw *'`). The label then
needs a line of the validation log tagged with the test's step (`[step] ...`, as a dagrun-style
runner prefixes each step's output) that contains that text, and a validation whose log cannot be
read gets no label from the rule.

## Ready sources and the request budget

Planning, validating and landing make no API calls. Heads arrive in one batched `git fetch` of
`refs/pull/N/head` (or of branches), and landing is a push. The hosting service's API is used
only by `sync --label` and `sync --all-open`, one list call for the whole queue, and by
`--close-prs`, one call per landed pull request.

Every API call is charged to a budget first: `--api-per-hour` requests per rolling hour, kept in
`api-budget.json` or in a shared `--api-budget-file`. A call the budget cannot cover is refused
before it is made, and the error says when room opens up. A list call that carries reviews
(`require_approval_at_head`) is charged 3 requests per hundred results instead of 1: it is still
one call, but its two nested review connections make the hosting service price it at about
three plain pages.

**Which ready source to use.**
- **`submit`.** When the agents that finish changes run on the same host as the queue, they should
  call `merge-queue submit` directly. It costs no API request, binds the exact head, and records
  which tests the agent ran.
- **`sync --label`.** Use it when people mark changes ready on the hosting service, or when you
  want the ready set visible there. It is also the ready source for a shadow / soak kit
  (`run --shadow` / `serve --shadow`) that should mirror the labelled ready set only: it costs
  one batched request per sync (plus one per label edit) and never widens to unlabelled pull
  requests.
- **`sync --file`.** Use it for a ready list kept by other tools. It costs no API requests.
- **`sync --all-open`.** For measurements that deliberately want every open, non-draft pull
  request; it is not the shadow-soak default, because it widens the queue beyond the labelled
  ready set.

On fleet hosts, `sync` and `--close-prs` reach the real GitHub CLI through the configured
`--gh` command. When no `--gh` is configured, the `FLEET_REAL_GH` environment variable is
honoured as a fallback (shell-style words), so a host that already exports the real `gh` path
there needs no extra flag; an explicit `init --gh` always wins. Git fetches themselves use the
clone's normal Git transport (only `github.*` API calls go through `gh`); pass
`--anonymous-fetch` for public repositories to keep fetches from spending credential uses. A
full portable validation run (`make validate-all`, i.e. `python3 scripts/validate.py --all`) is
separate from the queue's own validation: it checks this repository's contract and the queue
binary, while the queue's per-batch validation uses the validator you configured at `init`.

For public repositories, `--anonymous-fetch` keeps fetches from spending credential uses at all.

## Generated files

Some repositories commit files generated from sources, such as a validation graph compiled to
JSON or a static table compiled to code. Two changes that each regenerated such a file conflict
on it, even when their sources combine cleanly. If those files cause most of a backlog's rebases,
list them with `--regen-path` and give the command that rebuilds them with `--regen-command`.

- **Planning.** A conflict confined to generated files is not a conflict. The plan batches those
  changes together instead of holding all but one.
- **Building.** When combining a change leaves conflicts only in generated files, the queue first
  settles them on the target's side: where both sides edited the same lines the target's lines
  win, the change's other edits to the file are kept, and a conflict that is not about lines (one
  side deleted the file) takes the target's whole file, or its absence. No commit carries conflict
  markers, so a generator that compiles one of these files still builds. With `--recount-pins`,
  the pin file is recounted next, so generated files that embed the pins are made from the
  recounted values. The queue then checks the combination out in a scratch worktree, runs the
  command there, and commits the rewritten files on top as
  `merge-queue: regenerate <paths> after #<pr>`, under `identity`. It does this right after that
  change. It also regenerates after any change whose combination changed a
  generated file (a textual merge of generated files need not be the regenerated output) and,
  with `--regen-when always`, after every change; a regeneration commits only when a generated
  file actually changed. Because this happens per change, every prefix the culprit search might
  probe is a self-consistent tree, and the next change builds on the regenerated prefix, so the
  prefixes form a fast-forward chain for per-change land drivers.
- **Validation.** The candidate is then validated as usual. The commit that lands is the exact
  validated commit, regeneration included.
- **The regenerate contract (`--regen-command`).** The queue runs the command as `sh -c` with
  the working directory at the root of a scratch checkout of the candidate tip (the exact tree
  being combined), stdin closed, and these variables set: `MQ_PR` (the change being combined),
  `MQ_REGEN_PATHS` (the configured generated patterns, space-separated) and `MQ_CONFLICTED` (the
  generated paths that conflicted and were settled on the target's side, space-separated; empty
  when nothing conflicted). The
  command must rewrite every generated file deterministically from the checkout's sources and
  touch nothing else: for the same sources it must produce byte-identical output on every run,
  and the queue refuses any run that changes a file outside the generated set. A minimal correct
  example that rebuilds `gen/all.txt` from its sources is `mkdir -p gen && cat $(ls src/* | sort) > gen/all.txt`; a full example with paths and sources of your own follows the same shape — run in
  the candidate checkout, read `MQ_CONFLICTED` if you need to scope work, and never write outside
  the generated paths.
- **Refusals.** A change that cannot be regenerated is ejected with a kind that says why:

  | Kind | Meaning |
  |---|---|
  | `regen-build-failed` | `--regen-build` failed: the combined tree cannot even build the generator (stale against the base, or the combination broke the generator's sources). The reason carries the build error's last lines. |
  | `regen-failed` | The regeneration command itself failed (a wrong argument, a generator crash). |
  | `regen-touched-other` | The command changed a file that is not generated. |
  | `regen-left-conflict` | Conflict markers remain in a generated file afterwards. |

  Before blaming a change, the queue regenerates the tree just before it. If that fails too, the
  base or the generator's environment is broken, and ejecting each change in turn would eject them
  all; instead the run stops with the reason and nothing is ejected. A failure to start a
  process at all is likewise a run-level error, not an ejection. The kind is recorded on the
  ejection record, in the run report, in the per-change ejection file and in the notification the
  daemon sends.

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

## Pinned commits of other repositories

A repository can pin one commit of another in several places at once. The pin is a submodule
gitlink, and files repeat its hash: dependency manifests, lock files, build files. Two changes
that each move such a pin conflict at every one of those sites. A change based on an older pin
can also move it backwards. `pins` makes this deterministic: pins only move forward, and a change
is refused only when its pin cannot be put in order.

```json
"pins": [{
  "name": "dep",
  "gitlink": "third-party/dep",
  "files": ["*Cargo.toml", "*Cargo.lock", "tools/BUILD"],
  "url": "https://example.com/dep",
  "branch": "main"
}]
```

- `gitlink` is the submodule path whose commit is the pin.
- `files` names the files that repeat it, as `regenerate.paths` does: exact paths, directories,
  or `*` patterns. Every full 40-hex occurrence of a pin hash in them is a site. A declared file
  that carries no pin hash has no site, so a pattern may cover more files than carry the pin.
- `url` and `branch` (default `main`) name where pins are ordered. The branch is fetched into
  `refs/merge-queue/pins/<name>` only when a commit needed for that is missing.

When a change touches any site, by moving the gitlink or by editing a declared file, the queue
checks it after combining it onto the target:

- **Consistency.** Every declared file in the change that carries a known pin (the target's,
  the change's, or their merge base's) must carry the change's own gitlink commit and no other
  known pin. A file left at an older pin, holding two, or moved to a hash the gitlink does not
  name is a **partial update**: the change is ejected with kind `pin`, and the reason names
  each such file.
- **Order.** If the change moved the pin and the target pins a different commit, the
  descendant wins at every site. When the change's pin wins, it must be on `branch`. Pins where
  neither is an ancestor of the other (**diverged**), a pin missing from `url`, and a winning
  pin that is not on `branch` eject the change with kind `pin`. A change that keeps the pin its
  base had leaves the target's pin as it is, whatever it is.
- **Sites.** Conflicts in sites settle on the target's side while combining (planning treats them
  the same way, so two pin moves can share a batch). Each declared file that carries a pin, or
  whose conflict was settled, is then merged again: a three-way merge of the target's, the merge
  base's and the change's versions, with every known pin hash set to the winner. Pin hashes
  never conflict there, and no other edit of either side is lost. A conflict outside the pin
  remains a conflict and ejects the change. The gitlink is set to the winner. When anything
  changes, it goes in one commit on the candidate: `Pin <name> at <commit>`.

Each decision where the change moved the pin away from the target's is recorded in the run
report (`pin_resolutions`: `{pr, pin, target, change, chosen, sites}`), in the run's events,
and in the change's history (`pin-resolved`). A change that touches no site is never checked,
and nothing is fetched for it.

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
obligations, the pins resolved to the descendant (`pin_resolutions`; see "Pinned commits of other
repositories"), the batches a dry run `would_validate`, and a table of every validation: purpose
(`batch`, `probe`, `remainder`, `widen`), changes, commit, selection and policy, result (green,
red with the failed tests, or `cancelled`), `start_ms` (since the run began) and `elapsed_ms`,
and when known `parent` (the commit the changes were combined onto), `speculation`, `cpu_ms`
and `donor`. Each landing has its `at` time, and a culprit's ejection names the red validation
whose search `found_by` it. The
summary gives wall time, total validation time, `max_concurrent` (the most validations running at
once), the AIMD `windows` trace and `prs_per_hour` (changes landed per hour of wall time).
