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
- **Failed-test culprit search.** When a batch fails, the queue finds the change that makes the
  failure appear, and every probe runs **only the failed tests** (plus whatever the runner says
  those tests depend on). It never reruns the whole suite to look at a subset. By default the
  first red is the verdict: a red change on its own is ejected on that one sighting, and a red
  batch of several changes is not searched, because each of its changes goes back as a batch of its own. With
  same-commit confirmation on (`--confirm-reruns`), the queue searches the batch's prefixes, and a
  change is blamed only when its failure was seen twice, or once when the runner names every
  failure of that run a deterministic check (see "Deterministic checks"). The culprit is ejected
  with a report, and the queue continues with the rest. There is no automatic repair. See
  "Keeping the base green".
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
| `--max-depth N` | `0` | The largest the adaptive window may grow: +1 per landed batch, halved per red batch whose every failure timed out or reached no verdict, never below 1. Any other red leaves it as it is. `0` keeps the window fixed at `--depth`. |
| `--union-path PATTERN` | none | A Git attribute pattern merged with Git's `union` driver, when probing conflicts and when combining changes. Repeatable. Only for line-oriented, append-only files (see "Union merges"). |
| `--priority-label LABEL` | `mq-priority` | The label that makes a pull request a priority change when `sync` reads it from the hosting service, compared without regard to case. `-` turns it off. See "Priority changes". |
| `--classifier-policy PATH` | none | A test-footprint policy in the repository (version 2, such as `ci/test-footprints.json`), read at each plan's base commit. Decision-log features then take their components and a verdict from it, through the shared change classifier (`change-classify userguide`). |
| `--ci-path PATTERN` | none | A CI or validation-infrastructure path (exact path, directory with or without a trailing slash, or `*` wildcards). `plan` suggests the priority label for a change that touches only these paths; the queue never applies the label itself. Repeatable. |
| `--regen-path PATTERN` | none | A generated file (exact path, directory with or without a trailing slash, or `*` wildcards) whose conflicts the queue settles on the target's side and then regenerates, instead of holding the change. Repeatable. See "Generated files". |
| `--regen-command CMD` | empty | Shell command, run as `sh -c` at the root of the candidate checkout, that rewrites every generated file from its sources deterministically. Runs with `MQ_PR` (change number), `MQ_REGEN_PATHS` (space-separated generated patterns), `MQ_CONFLICTED` (space-separated conflicted paths, empty for a non-conflict refresh), `MQ_HEAD`, `MQ_PARENT` and `MQ_CHANGE_BASE` set (see [Generated files](#generated-files)). Required with `--regen-path`. |
| `--regen-when WHEN` | `conflict` | `conflict`: regenerate after a change whose combination conflicted in, or changed, a generated file (a textual merge of generated files need not be the regenerated output). `always`: regenerate after every change. Either way regeneration happens per change, so every prefix is a self-consistent tree. |
| `--regen-build CMD` | empty | Command run before `--regen-command`, such as building the generator. When it fails, the change is ejected as `regen-build-failed` with the error tail. |
| `--exact-sha` | off | Exact-commit rule: never carry a validation result to another commit. See "The exact-commit rule". |
| `--exact-merge-allowed` | off | Operator-asserted exception: exact merge commits may land even when the base branch's rules require linear history. Set only while an owner-granted ruleset exception is in place. |
| `--trust-base` | off | Treat the base as green in culprit searches and skip probing it. Sound when only this queue lands on the base, always as exactly validated commits; a broken base would then be blamed on the first change of a red batch. |
| `--classify PATTERN=CLASS[=EVIDENCE]` | none | Label failed tests whose ID, or whose step alone, matches `PATTERN` (exact or `*` wildcards) with `CLASS` in ejection records, reports and item history. With `EVIDENCE` (text with `*` wildcards), only when a line of the validation log tagged with the test's step (`[step] ...`) contains it. One example is `--classify 'test.*_count=count pin drift=expected * tests to execute, saw *'`. Repeatable. |
| `--recount-pins` | off | Recount the generated count pins (`ci/manifest-plan/src/validation_dag_static.rs`) after merging each change instead of treating their conflicts as real: each `("name", count)` entry becomes the base value plus every batched change's own delta (head minus its merge base), matched per name and occurrence. A pin a change introduces (absent at its merge base and at the base) takes the head's value; a pin the change leaves as it was keeps whatever the base did to it since, even one the base added or removed. A pin the change lowers (head below its merge base) lands only when the change's head commit declares exactly that decrease with a `Count-Pin-Decrease: <pin> <amount> <reason>` trailer (`<pin>` is a pin name for its first occurrence, or `name:N` for occurrence `N`; `<amount>` is written `2` or `-2`; one trailer per pin); an accepted declaration is recorded in the change's history (`count-pin-decrease`) and in the run report (`pin_decreases`). A change stacked on another (its head contains the other's head) carries the other's pin edits too, so it is measured from that head: from the other change's head when both are in the batch (its delta is its own minus the other's), and from the head of a change that already landed as a rebased copy or that a batch beneath it in the window carries (the batch is built on that batch's candidate). The shared edits count once, and the stacked change declares only the decreases it makes itself: counts come from the pin values in the commits, never from a trailer, so a trailer on the stacked change for the other change's decrease is ignored. A change whose head an earlier change in the batch already contains adds nothing; one stacked on several changes that are not stacked on one another is skipped until they land. A change is skipped from its batch and stays queued when its pins cannot be recounted (the file missing in a version, a pin it changes whose occurrence count differs across versions, or a decrease that is undeclared or differs from its declaration; that refusal names the pin's merge-base and head values, so a removed test can be told from a stale pin, and the trailer that would declare it). A change whose new pin is lost settling a conflict in the pin file is ejected as a conflict naming that pin. |
| `--peer NAME=DIR` | none | A peer queue: another repository's merge-queue state directory. Repeatable. See "Changes across repositories". |
| `--identity-name NAME`, `--identity-email EMAIL` | Git's configuration | Identity for commits the queue creates: committer of replayed commits, author and committer of merge and regeneration commits. |
| `--github-repo OWNER/NAME` | empty | Hosting repository for `sync --label`, `sync --all-open` and `--close-prs`. |
| `--gh WORD` | `gh` (or `$FLEET_REAL_GH`) | Command (and leading arguments) to run instead of `gh`, for example a pacing wrapper. Repeatable. When no `--gh` is configured, the `FLEET_REAL_GH` environment variable (shell-style words naming the real `gh` on fleet hosts where `gh` on `PATH` may be a stub) is used as a fallback; an explicit `--gh` always wins. |
| `--api-per-hour N` | `0` (= 60) | API requests this queue may make per rolling hour. See "Ready sources and the request budget". |
| `--api-budget-file PATH` | state directory | Budget file to share with every other queue on the host, so they all draw on one hourly allowance. |
| `--anonymous-fetch` | off | Fetch without credentials (public repositories). Credential helpers are disabled for fetches, so a fetch never spends a credential use. |
| `--require-approval-at-head` | off | For `sync --label`/`sync --all-open`: a pull request counts as ready only when a reviewer with write access (author association `OWNER`, `MEMBER` or `COLLABORATOR`) has, as their latest verdict, approved exactly its current head commit (full commit id; comments do not replace a verdict), no such reviewer's latest verdict requests changes, and the hosting service's `reviewDecision` is not `CHANGES_REQUESTED` or `REVIEW_REQUIRED`. Missing review data never approves. Reviews are read in the same batched list call (no per-pull-request calls), charged 3 requests per hundred results instead of 1 because of the nested review connections. An unapproved change is not submitted; a queued change whose head moves without a fresh approval at the new head is withdrawn and its submitter notified (an existing `--notify` subscription is kept; the author is only the default). Resubmission with an approval at the new head re-queues it. Ready-file sources are unaffected. |
| `--require-approval-at-head` | off | For `sync --label`/`sync --all-open`: a pull request counts as ready only when a reviewer with write access (author association `OWNER`, `MEMBER` or `COLLABORATOR`) has, as their latest verdict, approved exactly its current head commit (full commit id; comments do not replace a verdict), no such reviewer's latest verdict requests changes, and the hosting service's `reviewDecision` is not `CHANGES_REQUESTED` or `REVIEW_REQUIRED`. Missing review data never approves. Reviews are read in the same batched list call (no per-pull-request calls), charged 3 requests per hundred results instead of 1 because of the nested review connections. An unapproved change is not submitted; a queued change whose head moves without a fresh approval at the new head is withdrawn and its submitter notified (an existing `--notify` subscription is kept; the author is only the default). Resubmission with an approval at the new head re-queues it. Ready-file sources are unaffected. |
| `--close-prs` | off | After landing, close each landed pull request with a comment naming the landed commit. The close is made beside the run, never in its loop: it uses the budget, which can make it wait minutes, and the next batch does not wait for it. A close it cannot make now is retried at the next landing and by `sync`. |
| `--agentctl WORD` | `agentctl` | Command (and leading arguments) the `serve` daemon runs instead of `agentctl` for state-change notifications. Repeatable. |
| `--agentctl-registry PATH` | none | agentctl registry the notifications address: sends run `<command> --registry PATH send ...`. Stored absolute as `notify.registry`. Without it agentctl uses `.agentctl` in the daemon's working directory, which for a daemon started in a bare queue repository is no registry, so every send fails. |
| `--net-wrapper WORD` | none | A command prefix for network Git operations, for example `--net-wrapper with-proxy`. Repeat it for a prefix of several words. |
| `--net-timeout-secs SECS` | `0` (300) | How long one network command (`git fetch`, `ls-remote`, `push`, a `gh` call) may run before its process group is killed and it fails (see "Network commands are bounded"). |
| `--precheck CMD` | empty | Shell command every submitted head must pass before it is queued, run on the head alone (a formatter or documentation check, say). A head that fails it is ejected at submission with the command's output. Empty: no precheck. See "The precheck". |
| `--precheck-timeout-secs SECS` | `0` (300) | How long one precheck may run before its process group is killed; the check then counts as one that could not run, and the change is queued without it. |
| `--precheck-background CMD` | empty | Shell command for checks too slow to wait for (a documentation build, a linter that compiles), run on every queued head by a worker beside the run. A head whose verdict is a failure is ejected when its batch is built; one not checked yet joins the batch. Empty: none. See "Slow checks in the background". |
| `--precheck-background-timeout-secs SECS` | `0` (1800) | How long one background precheck may run before its process group is killed; the check then counts as one that could not run. |
| `--precheck-covers STEP` | none | A validation step the precheck commands reproduce (repeatable). When every failure of a red batch is such a step and a change in it failed its precheck, that change is ejected and the rest go back as one batch. |
| `--validator KIND` | empty | `command` or `dagrun` (see "Validators"). `run` refuses to start without one. |
| `--validator-run CMD` | empty | For `command`: the shell template that runs a selection. |
| `--select CMD` | empty | Shell command printing the selection policy's test IDs for the checkout's HEAD relative to `$MQ_BASE`. Empty: every validation runs the complete suite. |
| `--id-prefix TEXT` | empty | Only lines of `--select` output that start with this prefix are test IDs (the prefix is removed), for example `STEP `. |
| `--policy NAME` | empty | The name recorded for the selection policy (empty records `select-command`, or `all` without `--select`). |
| `--dag FILE` | empty | For `dagrun`: the DAG file, relative to the candidate checkout. |
| `--dagrun PATH` | empty | For `dagrun`: the dagrun executable, relative to the checkout or absolute (empty means `dagrun` on `PATH`). |
| `--admission WORD` | none | A command prefix every validation runs under, such as a host admission wrapper. Repeat it for several words. |
| `--refusal-max-retries N` | `3` | Extra attempts after an admission refusal (exit `75`, or a non-zero exit whose first output line starts with `COULD_NOT_RUN` and that attributed no failed test) within one `run`, each after a backoff delay. Each attempt is a validation of its own beside the run's others, started when the refusal comes back rather than when the batch reaches the head, so the run goes on taking requests and landing meanwhile. A refusal is never a pass and never red: the change stays queued. |
| `--refusal-backoff-secs SECS` | `2` | Base backoff between refusal retries, doubled per attempt and capped at 60 seconds. `0` means no delay. The same backoff sets the deferred change's next retry time. |
| `--max-deferrals N` | `3` | How many times a change may be deferred because validation did not run (an admission refusal, a validator that cannot execute, a probe or donors that could not run) before it is escalated to its owner and held until `merge-queue retry` (see "Bounded retry"). |
| `--green-flaky-hold-secs SECS` | `7200` | How long a change is held for a ruling after a GREEN_FLAKY or INFRA-BLOCKED result (see "Held for a ruling") before it is re-queued for a fresh validation. |
| `--green-flaky MODE` | `hold` | What a GREEN_FLAKY result does when its retried tests are not all on the known-flaky list: `hold` it for a ruling, or `land` it at once with the retried tests recorded (`validator.green_flaky` in the config; absent means `hold`, and an older binary refuses a config that names it). INFRA-BLOCKED always holds. |
| `--red-without-probe MODE` | `eject` | What a red validation that names failed tests does when its follow-up probe cannot run: `eject` the change (the red run is the verdict) or `hold` it for its owner. |
| `--empty-red MODE` | `red` | What a red run that recorded no test result at all (no result lines, no verdict lines, nothing ran) does: `could-not-run` treats it as a refusal (no strike, requeued at the same head, counted against `--max-deferrals`), or `red` (the default) keeps it as a red verdict, named as a red with no recorded test result when it ejects. See "A red with no recorded test result". |
| `--fail-fast` | off | Ask the runner to stop each candidate validation at its first red step: `$MQ_FAIL_FAST` is `1` for every validation except the complete run on the base (see "Fail-fast runs"). |
| `--peripheral-command CMD` | off | Consumer hook marking changes peripheral (see the `peripheral` fields). |
| `--peripheral-timeout-secs N` | `30` | Seconds one classifier call may run before the change counts as binary-touching. |
| `--peripheral-max-batch N` | twice `--max-batch` | Largest batch of only peripheral changes. |
| `--confirm-reruns` | off | Confirm a red before blaming a change: search a red batch's prefixes, and rerun a failure at the same commit (`$MQ_RERUN_FROM`), so that a failure is blamed only when seen twice. Off, the first red is the verdict: a red change on its own is ejected on that sighting, and each change of a red batch goes back as a batch of its own (see "Keeping the base green"). |
| `--suspect-flake-reruns` | off | With confirmation off, hold a single change whose red run failed only on suspect tests (timeouts, or runs that reached no verdict, as the runner shapes its results) instead of ejecting it, run those tests again at its commit beside the run, and re-queue it if they pass or eject it, seen twice, if they fail again. Once per head. See "Suspect flakes". |
| `--suspect-flake-timeout-secs SECS` | `0` (1800) | Wall limit of one suspect-flake rerun. |
| `--split-red-batches` | off | With `--confirm-reruns`, still send each change of a red batch of several changes back as a batch of its own instead of searching the batch's prefixes (a search holds the run for its whole length). Each red single-change batch is then confirmed by one rerun of its failed tests at the same commit before its change is ejected. No effect without `--confirm-reruns`. |
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

With a precheck configured, the head is checked before it is queued (see "The precheck"). A head
that fails is ejected at once: `submit` exits non-zero and prints the command's output, and a
recorded submission shows the refusal under `intake refused` in `status`.

| Argument | Default | Meaning |
|---|---|---|
| `PR` | required | The change number (pull-request number). It must be positive. |
| `--head REF` | required | The head branch on the remote, or a full ref such as `refs/pull/42/head`. |
| `--sha SHA` | empty | The exact head commit (or a prefix of it) that you mean. Submission is refused if the branch points elsewhere. |
| `--priority N` | `100` | Lower values land first. |
| `--after PR` | none | A change that must land first. Repeatable. While it (or a change it must land after) is ejected, withdrawn or not in the queue, this change is held: it stays queued, is never validated or landed without it, and `submit`, `list` and `status` say `held: waiting on #PR (ejected)`. It is planned again once #PR is queued and lands first (resubmit an ejected or withdrawn #PR with `submit`); resubmitting this change without that `--after` drops the dependency. Held, never rejected: the dependency can be ejected after this change was accepted, and a dependent may be submitted before its dependency. |
| `--solo` | off | Validate and land this change in a batch of its own. Use it for risky changes. |
| `--prioritized` | off | A priority change, as the priority label makes one under `sync`: planned ahead of all other work and validated in a batch of its own. It still lands only on its own exact green. See "Priority changes". |
| `--exact` (alias `--preserve-merge`) | off | Land the head exactly as submitted, never replayed. A merge commit keeps both parents, so the history it merges becomes an ancestor of the base (for example, importing another repository's history). The head must be a fast-forward of the base: commits linear on top of it, or a merge whose first parent is the base itself. If the base moves before it lands, the change is ejected as not fast-forward and nothing is replayed: rebuild it on the new base, revalidate, and resubmit. It is validated and landed in a batch of its own, and the queue never rewrites it (pins, recount, regeneration; a regeneration that would change it ejects it). Once set, resubmitting keeps it. Without `--exact`, a merge head is replayed as its non-merge commits. When the head is a merge commit, submit (and the run, before validating) reads the base branch's effective rules: if they require linear history, the submission is refused (or the queued change held) naming the rule, unless `exact_merge_allowed` is set; if the rules cannot be read, submit warns and says so in its output. |
| `--by NAME` | empty | Who is submitting (an agent or a person). |
| `--notify NAME` | empty | agentctl session name the `serve` daemon notifies exactly once per state change (landed, ejected, refusal-deferred, escalated, withdrawn). Stored on the item next to `submitted_by`; resubmitting without it keeps the recorded name. Ready files set it per line with `notify=NAME`. |
| `--notify-mode MODE` | config | Per-change override of the queue's `notify_mode`: `both`, `hooks-only` or `notify-only`. Stored on the item; resubmitting without it keeps the recorded override. See "Event hooks". Ready files set it per line with `notify-mode=MODE`. |
| `--title TEXT` | empty | A one-line description. |
| `--tested-with SELECTION` | none | The tests you ran on this head: `all`, a comma- or space-separated list of test IDs, or `@FILE` with one ID per line (`#` starts a comment). This becomes the change's validation claim. Without it, the change has no claim. |
| `--policy NAME` | `manual` | The name of the policy that chose `--tested-with`, for example `path-routing`. |
| `--requires SPEC` | none | A commit in a peer repository that must be on that repository's base first: `PEER#PR`, the head of that peer change (which is then marked to land as its own commits), or `PEER@SHA`. Repeatable. See "Changes across repositories". |
| `--skip-precheck` | off | Queue the head without the precheck, for a refusal that is wrong. The change's history records that the check was skipped. |

The output names the head and, when there is a claim, says whether it was focused or complete:
`queued #42 at 9b1c0e77aa (priority 50); validated with 2 tests (focused)`.

### `merge-queue sync`

Reconcile the queue with a ready source. Exactly one of these is required:

| Option | Default | Meaning |
|---|---|---|
| `--label LABEL` | none | Ready = open pull requests carrying `LABEL`, in one batched API call. |
| `--all-open` | off | Ready = every open, non-draft pull request targeting the base, in one batched API call. |
| `--file FILE` | none | Ready = the changes listed in `FILE`, one per line. Makes no API call. |
| `--limit N` | `300` | Most pull requests to list. Each 100 costs one API request. A listing that reaches `N` may have been cut short, so it withdraws nothing it did not list (the report sets `truncated`). |
| `--dry-run` | off | Report what would change without submitting or withdrawing anything. The list call is still made. |
| `--json` | off | Print the report as JSON. |

`sync` submits new changes and changes whose head or priority moved, with one fetch for all of
them. It withdraws only changes that this same source added and no longer lists, and only when
the listing is complete: a listing as long as `--limit` may have stopped before a change that
is still ready. A change you submitted directly is never withdrawn by a source. Draft pull
requests are never ready.

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
holds the change; a change whose submission is still recorded can be withdrawn the same way (see
"Intake while a run holds the queue").

### `merge-queue retry PR...`

This command returns changes that were escalated to their owner (see "Bounded retry") to the
queue: it clears the escalation, the refusal count and the backoff, in the item records and in
the shadow ledger. Run it once whatever kept validation from running is fixed. A change that is
only deferred is returned at once too, without waiting for its backoff. It is also the ruling
that ends a hold (see "Held for a ruling"): the change is validated again. While a run holds the
writer lock, the retry is recorded and applied when the lock frees (see "Intake while a run holds
the queue").

### `merge-queue accept-flaky PR... --by WHO --reason WHY`

This command is the ruling that accepts the GREEN_FLAKY passes of a held batch (see "Held for a
ruling") without validating it again. Name every change of the held batch, in any order, and
only those; the hold must be a GREEN_FLAKY hold of a green batch, which records the candidate it
validated. `--by` and `--reason` are required and go in each change's history (`accepted-flaky`)
and in the decision log (`decisions/rulings.jsonl`, see "The decision log").

A run then acts on the ruling, before it builds anything new on the base:

- If every change is still queued at the head that was validated, the base is still the commit
  the candidate was built on, and the candidate is still in the queue's repository, the run lands
  exactly that commit, with the retried tests recorded as flaky and the ruling noted with the
  landing. The earlier validation is the evidence for the exact-commit rule. Batches in flight
  were built on that base without the held batch, so they are cancelled first and built again
  on top of it.
- Otherwise the hold and the ruling are cleared (`accepted-flaky-requeued`, with the reason) and
  the batch is validated again like any queued change.

While a run holds the writer lock, the ruling is recorded (see "Intake while a run holds the
queue"), and the run acts on it as soon as its window has a free slot, or when it ends. A
refusal names what is wrong: a part of the batch, a change not held, an INFRA-BLOCKED hold (there
is no pass to accept; `retry` once the host is fixed), or a hold that recorded no candidate (a
red batch's rerun, or a hold made by an older version); `retry` validates those again.

### `merge-queue known-flaky add TEST --issue ISSUE --by WHO`

The known-flaky list holds tests whose flakiness is known and tracked by an issue. It is a
standing ruling: a GREEN_FLAKY result whose retried tests are all on the list lands at once,
without a hold, the tests recorded as flaky with the landing, and the ruling goes to the decision
log as `known-flaky`. A result that retried any test not on the list is held as before — unless
`validator.green_flaky` is `land`, in which case it lands by that policy instead (see
"Held for a ruling").

- `add TEST --issue ISSUE --by WHO [--reason WHY]` adds the test, or replaces its entry. `ISSUE`
  is a link or a tracker ID. An entry covers its test, the test's cells (`TEST[cell]`, as a
  runner that runs one test in several configurations names them) and its cases
  (`TEST::case`); an entry for one cell covers that cell only.
- `remove TEST --by WHO` removes the entry for exactly `TEST`.
- `list [--json]` prints the list (schema `merge-queue-known-flaky/v1`).

The list is `known-flaky.json` in the state directory, replaced atomically, so a run reads either
the old list or the new one. A list that cannot be read covers nothing (an event says so), so
results are held as if it were empty.

### Intake while a run holds the queue

A run holds the writer lock from start to finish, which can be many minutes. `submit`,
`withdraw`, `retry` and `accept-flaky` never wait for it and never fail because of it. When the
lock is busy,
each one records its request as a file under `intake/pending/`, prints `recorded ...` and exits 0.
The next holder of the lock applies the recorded requests in the order they were recorded,
before it reads the queue. That holder is the `serve` daemon at its next poll, a `run`, a
`sync`, or the next `submit`, `withdraw` or `retry` that finds the lock free. A recorded request
does exactly what the command would have done then, and its history entry notes when it was
recorded.

The run holding the lock does not make every request wait for its end. Whenever its speculative
window has a free slot and the run goes on (a batch is in flight, or batches are still to
start), it applies the requests for the changes it is not busy with: every change except those
in flight and those it ejected or set aside. It then plans every queued change it has not
started again, on top of the last batch in flight. A new change can then start validating in
the free slot at once, and a `--prioritized` one goes before batches that have not started. A
batch that came back red behind the head holds no place in the window, and the batches built on
it are cancelled (see "Keeping the base green"), so those slots take requests at once, built
beneath the red batch. The run checks for requests every `--poll-ms` under `serve` and every 15
seconds under `run`.

- The plan is made on the candidate of the last batch in flight, so a change that conflicts
  with work in flight is held for the next run, not ejected. A change that fails only when its
  batch is built on a batch in flight (a conflict in a regenerated file, a generator that fails
  on the combined tree) is set aside the same way, with the changes that depend on it: its
  history says `set-aside`, it stays queued, and the next run builds it on the base. It is
  ejected only if it does not combine onto the base itself, so a change is never ejected for a
  conflict with a change that never lands. A change stacked on a change in
  flight, or submitted `--after` one, is planned on top of it and depends on it: if that change
  is ejected, the stacked one is set aside, never rebuilt without it.
- Landing order does not change. A batch planned this way lands only after every batch beneath
  it, and a red batch beneath it sends it back to be built again, as for any batch.
- Requests for a change in flight wait for the run to end (a withdrawal still keeps its batch
  from landing, below). A `retry` of a change the run holds, escalated to its owner or held for
  a ruling, does not: it applies at the next free slot, and the run plans the change again on
  top of the batches in flight. All requests wait during a drain, and after a run has been taking
  requests for two hours: it then finishes the batches in flight and ends, so the next run
  reloads the configuration and the validator's tools. The run report lists what the run
  applied under `intake`.
- Two more requests apply at the next intake point even though the run set their change
  aside. A `withdraw` of a change with no running validation (held for a ruling, say)
  applies at once and the run goes on without it. And a `submit` at a *new* head for a
  change the run ejected or set aside applies at once too: the exclusion was for the old
  head, so the stale exclusion is dropped and the new head is planned on top of the
  batches in flight, in the same run. A `submit` at the *same* head the run ejected keeps
  waiting for the run to end, and its `pending-intake` line says why: `waits for the run
  end: excluded this run (ejected at <head>)`.
- A held front item (held for a ruling, not validating) does not block the greens behind
  it: batches built on the base itself that do not depend on it land in the same run as if
  it were absent, and the run logs `landing blocked behind #N (held): ...`. Changes built
  on the held item's tree, and changes submitted `--after` it, still wait for it. `status`
  names the block (`landing blocked behind #N (held LABEL until T): K green waiting
  (#M, ...)`) when a kept green sits behind a held change.

- `submit` first checks, without writing to the repository the run is using, that the head
  resolves (`git ls-remote`), and that it matches `--sha` when one is given. A head that does
  not resolve is refused at once, as it would be with the lock free.
- `withdraw` and `retry` first check that the change is in the queue (and, for `retry`, queued).
  A change whose submission is itself still recorded counts for `withdraw`: the withdrawal is
  recorded behind that submission and applied right after it, so a bad head can be stopped
  before the change is planned. A withdrawal also takes effect inside the run that holds the lock: before landing a batch, the
  run reads the recorded withdrawals. If one names a change in the batch, the run does not land
  that batch, stops, and leaves the batch queued. The next cycle applies the withdrawal and
  validates the rest again.
- A request that cannot be applied when its turn comes (its change landed meanwhile, its head
  no longer resolves) is moved to `intake/refused/` with the reason, noted in the change's
  history (`intake-refused`), and shown by `status` for a week.
- A recorded request is not in any item record until it is applied, so `list` and `status` show
  it on its own line, for example `#4885 pending-intake: submit pr-4885 at 1a2b3c4d5e since
  2026-11-04T20:51:03Z` (`list --json`: a row with `"status": "pending-intake"`).
- Asking again is harmless. A request identical to the last one waiting for the same change is
  not recorded again: the command prints `already recorded #N at <time>` and exits 0. Identical
  requests recorded one after another anyway (two submitters at the same moment) are applied
  once, and the log line says how many were folded into it.

### `merge-queue list`

This command prints the queued changes as a table with the columns `PR`, `STATUS`, `PRIO`, `HEAD`,
`TESTED` and `TITLE`. `TESTED` shows the scope of the submitter's claim (`complete`, `focused`,
or `-` when there is none), so focused submissions are visible at a glance. `PRIO` ends in `!` for a
priority change. A queued change held for a change it must land after (`--after`) has a
`held: waiting on #N (ejected)` line under its row (`list --json`: `waiting_on`, with `through`
naming the queued changes in between).

| Option | Default | Meaning |
|---|---|---|
| `--all` | off | Include landed, ejected and withdrawn changes. |
| `--json` | off | Print the full item records as a JSON array, followed by one row per request recorded while a run held the queue (`pr`, `"status": "pending-intake"`, `op`, `head_ref`, `head`, `recorded_at`, `last_recorded_at`, `requests`). |

Requests recorded while a run held the writer lock (see "Intake while a run holds the queue") are
listed after the table as `#N pending-intake: <op> ... since <time>` until they are applied, with
the number of identical requests when a submitter asked more than once.

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
| `--sync-label LABEL` | none | Also ingest pull requests carrying LABEL, on a thread beside the daemon's loop: the same ingestion as `merge-queue sync --label LABEL` (one batched list call, approval at head when configured, withdrawal when the label goes away). Nothing waits for it. Needs `github.repo`. |
| `--sync-every SECS` | `60` | Seconds between label syncs (with `--sync-label`; minimum 10). Each sync spends at least one hosting API request. |
| `--sync-limit N` | `100` | Pull requests one label sync lists at most (with `--sync-label`). Each 100 costs one request of the budget, three with `require_approval_at_head`, so the default sync costs 1 or 3. A sync that lists `N` may have been cut short, so it withdraws nothing it did not list, and its summary says so. |
| `--offload MODE` | configured | Override `validator.offload` for this run: `off`, `compare` or `decide`. |

With `--sync-label LABEL` the daemon ingests labelled pull requests itself, so a change
labelled ready that nobody submits is never missed. Each sync runs on its own thread, started
when one is due and none is in flight. Its list call can wait minutes on a paced or slow
hosting service, so the daemon never waits for it: from the first cycle after startup it
plans and validates what is already queued, takes recorded requests and keeps its heartbeat
fresh, and the heartbeat's `sync_started_at` shows a sync in flight. When the listing
arrives:

- If the writer lock is free, the sync submits and withdraws under it (never a second
  writer), and the next loop iteration validates what it submitted.
- If a run holds the lock, the sync records its submissions as requests for that run, which
  takes them into a free slot of its window (see "Intake while a run holds the queue").
  Withdrawals wait for a sync that gets the lock.
- Landed pull requests whose close the request budget postponed are closed without the lock.
  Each close is noted under `<state>/closed/`, and the next holder of the lock records it on
  the change.

A failed sync (the hosting service down, the request budget spent) is logged once per distinct
error and recorded as `sync_error` in the heartbeat and `status`. It never stops validation of
what is already queued. A successful sync records `last_sync_at` and a one-line summary.
Notifications its withdrawals owe go out through the daemon's own `--agentctl` and send
timeout. `serve --once` waits for its one sync, so that single cycle includes it.

With `--shadow` the daemon is a soak harness: it plans, validates and would land exactly as a
live daemon would, but every landing advances only the in-memory simulated base. Run reports
carry `mode: shadow`, the heartbeat carries `shadow: true`, and `merge-queue status` shows
`mode: shadow (landings simulated; real base unchanged)` so a shadow daemon is never mistaken
for a live one. Because no item really lands, no `landed` notification is owed in shadow mode;
the notification sweep still runs each cycle for the transitions that are real (withdrawals,
refusals). A queue whose `land.driver` is `none` refuses live cycles with a remedy naming
`--shadow`; `serve --shadow` is that remedy.

## Event hooks

For push-based, low-latency reactions — including when the notified agent is gone or in a
long turn — `hooks` in `config.json` names a shell command per event. Each is called with a
versioned JSON payload on stdin (`$MQ_HOOK_EVENT` and `$MQ_HOOK_PR` in the environment),
fire-and-forget: a bounded worker pool runs hooks with a per-hook timeout
(`hooks.timeout_secs`, default 30), the queue never waits on one and never fails because of
one, and every finished hook is logged to `<state>/hooks.jsonl` with its exit status,
duration, and stdout/stderr tails. By default the existing `--notify` delivery is
unchanged beside them; `notify_mode` (next paragraph) can make hooks the only
delivery for the events they cover.

| Field | Event |
|---|---|
| `on_red` | A red batch verdict (PRs, heads, failing nodes, log paths). |
| `on_eject` | A change was ejected. |
| `on_held` | A GREEN_FLAKY hold for a ruling. |
| `on_land` | A change landed (with `landed_sha`). |
| `on_base_moved` | The base moved (`old_base`, `new_base`). |
| `on_intake_refused` | An intake request recorded while the writer lock was busy was refused. |
| `on_escalated` | A change was escalated to its owner. |

Every field absent (or empty) is off. The payload is `merge-queue-hook/v1`:

```json
{"schema":"merge-queue-hook/v1","event":"on_land","at":"...","pr":1,
 "prs":[1],"head":"<sha>","shepherd":"alice","notify":"alice",
 "landed_sha":"<sha>","old_base":"<sha>","new_base":"<sha>",
 "failing":["tests/a"],"logs":[".../run.log"],"culprit":"tests/a",
 "reason":"...","detail":"..."}
```

Fields that do not apply to an event are empty. A per-item `shepherd` — the agent driving a
change to landing, set with `submit --shepherd NAME`, changed with
`merge-queue shepherd PR NAME` (or `--clear`) — is carried in every payload for that item,
separate from `notify` (who gets reports). Each payload also carries the item's effective
`notify_mode`.

Which channel reports a change is set by `notify_mode` in `config.json` — one of `both`
(the default), `hooks-only` or `notify-only`, set at `init --notify-mode` — and overridable
per change with `submit --notify-mode MODE` (a ready file spells it `notify-mode=MODE`).
A resubmission without the flag keeps the recorded override. In `both` mode hooks and the
built-in `--notify` delivery each fire independently, exactly as before. In `hooks-only`
mode the built-in delivery is skipped for a verdict that has a configured hook — `landed`
for `on_land`, `ejected` for `on_eject`, `escalated` for `on_escalated`, `held-green-flaky`
for `on_held` — and still fires for any other verdict (for example `withdrawn` or
`refusal-deferred`, and a landing when no `on_land` is configured), so a change is never
silently dropped when its event has no hook. `notify-only` is the mirror: a hook whose
verdict notify reports is skipped for a change that has a `--notify` target, and still
fires otherwise (including `on_red`, `on_base_moved` and `on_intake_refused`, which notify
never reports). The effective mode is recorded in the change's `submitted` history event,
shown as `notify_mode` by `list --json`, and named in the `notify-suppressed` history
event (with the hook that is the delivery) whenever `hooks-only` suppresses a send. A
router that consumes hooks and routes events itself — suppressing infrastructure reds,
say — runs its queue in `hooks-only` so the built-in delivery does not bypass it.

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
batches a live daemon's heartbeat names: the batches its run built and is validating or deciding
(a change the run skipped while building a batch is not among them), none while it idles, and
none when no daemon is running (a `run` started by hand reports its own progress). Slots count the `scratch/slot-*`
checkouts; one is in use only while a validation holds its `scratch/slot-<n>.lock`, since every
slot keeps its checkout between validations. A failed attempt to take the writer lock shows as
the daemon's last error until the daemon gets the lock or has nothing queued. Requests recorded
while a run held the lock show as `intake: N request(s) ...` until they are applied, one
`#N pending-intake: ...` line per change and request (JSON `intake.requests`). Requests
refused when applied show as `intake refused: ...`, and the newest three are named (JSON `intake`).
A pending drain request shows as `drain: requested at ...` (JSON `drain`; see **Upgrading the
daemon** below). A queued change held for a change it must land after shows under the queue
counts as `#N held: waiting on #M (ejected)` (JSON `waiting`; see `--after` under `submit`).

| Option | Default | Meaning |
|---|---|---|
| `--json` | off | Print the status report as JSON (`daemon`, `pid`, `heartbeat`, queue counts, `active_batches`, slots, `intake`, `drain`, `waiting`, `remedy`). |
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

### Upgrading the daemon (`merge-queue drain-for-upgrade`)

Stopping or restarting the daemon cancels the validations in flight, and the daemon that replaces
it validates those batches again from scratch: a restart in the middle of a long validation
throws that validation away. When validations run detached from their launcher (as their own
service units, say), the cancelled one would otherwise keep running with nobody reading its
verdict, holding its validate slot: cancelling a donor job that the *local* donor is running
also stops that job's `validate-merge-queue-*` unit (by unit name, from the queue side and
from the donor's own next renewal), and a daemon that was killed outright leaves orphans the
next `serve` stops at start, before it admits any new work, logging each one. The stopping
daemon's exit log names any local validation unit it still leaves running. A drain avoids
the wasted work entirely. The running daemon finishes the validations already in flight and
acts on their verdicts (a green batch lands), starts no new validation (no new batch, no
culprit-search probe, no rerun), and exits 0 once no run is in progress. The changes it did not
decide stay queued, and nothing is ejected for the drain. A red batch whose culprit search, or
whose changes split off one by one, the drain left undecided is planned and validated again by
the next daemon.

```bash
install -m 755 merge-queue.new /path/to/merge-queue   # the running daemon keeps its own copy
merge-queue drain-for-upgrade --reason "new binary" --wait --timeout 3600
systemctl --user start merge-queue-serve               # Restart=on-failure does not restart an exit 0
```

The request is `serve.drain.json` in the state directory and names the daemon's pid. Only that
daemon honours it, and a daemon removes any request on file when it starts, so a request left by
a daemon that died never drains its replacement. An idle daemon exits within one poll; a daemon
in a run exits when the run ends.

| Option | Default | Meaning |
|---|---|---|
| `--reason TEXT` | empty | Kept with the request, shown by `status`, and written to the daemon's log. |
| `--wait` | off | Wait for the daemon to exit, checking every second, then exit 0. |
| `--timeout SECS` | `7200` | With `--wait`: seconds to wait before giving up with exit 1. The request stands. |
| `--cancel` | off | Withdraw a pending request instead. |

With no daemon running, `drain-for-upgrade` exits 1: there is nothing to drain.

### `merge-queue donor`

Donor validators (see "Donor validators"). `donor run` is the donor daemon, run on any host with
a clone of the queue's repository. The other verbs are what a donor calls on the queue host
through its route; an operator uses only `status`.

| Verb | What it does |
|---|---|
| `run --queue CMD --clone DIR --name NAME [--jobs N]` | Claim jobs through the route, validate each exact candidate in a worktree of `DIR`, report receipts. Options: `--remote` (default `origin`; fetched for the base), `--poll-secs` (default 5), `--work DIR` (default `merge-queue-donor/` in the clone's Git directory), `--mismatch-backoff-secs` (default 300), `--until-idle`, `--cap-file PATH` (read before every claim: a whole number N caps `--jobs` at N, 0 drains, claiming nothing and never exiting under `--until-idle`, and an absent or malformed file means `--jobs`; running jobs are never touched, and each change is logged). SIGINT/SIGTERM gives running jobs back and exits. |
| `status [--json]` | Jobs by state, leases (never their tokens), and in `compare` mode how many receipts matched the local verdict. |
| `claim --donor NAME [--host H] [--jobs N] [--drain]` | Lease the oldest job this donor may take; prints `{"job": ...}` (with the token) or `{"job": null}`. The cap it claims under (0 with `--drain`, which leases nothing) is kept for `donor status`. |
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
   A dependent of a held change is held as `waiting-on-dependency` as well. A run that ejects a
   change sets its dependents aside (`requeued` in the run report); they are never validated
   without it. `list` and `status` show such a change as `held: waiting on #N (ejected)`, or
   `held: waiting on #M, which waits on #N (withdrawn)` through a queued change (`list --json`:
   `waiting_on`; `status --json`: `waiting`).
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
  serve.drain.json       fixed: a pending `drain-for-upgrade` request, naming the daemon's pid
  union.attributes       fixed: generated from union_paths, when set
  items/<pr>.json        one record per submitted change
  notifications/        notification report files the daemon sends with `--file`
  runs/<run-id>/         one directory per run (layout below)
  decisions/<day>.jsonl  the decision log: one record per validation, by UTC day (see "The decision log")
  decisions/skips.jsonl  one record per change left out of its batch
  decisions/rulings.jsonl  one record per ruling on a GREEN_FLAKY batch
  known-flaky.json       the known-flaky list (see `merge-queue known-flaky`)
  obligations/<id>.json  one record per focused landing still owed a complete run
  scratch/slot-<n>/      one reused worktree per concurrent validation
  scratch/slot-<n>.lock  flock held by the run while a validation uses slot <n>
  scratch/precheck/      the reused worktree every precheck runs in
  scratch/precheck.log   the output of the last precheck
  scratch/precheck-background/     the background worker's own reused worktree
  scratch/precheck-background.log  the output of its last check
  scratch/suspect/       the suspect-flake worker's own reused worktree
  suspect/<pr>.json      the verdict of a change's suspect-flake rerun, until a run acts on it
  suspect/<pr>-<commit>/ that rerun's log and results
  precheck/verdicts/<head>-<digest>.json  one precheck verdict per head and command (kept 14 days)
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
holds it. `submit`, `withdraw`, `retry` and `accept-flaky` record their request when it is held
(see "Intake while a run holds the queue").

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
- `held_candidate`: with a green batch's GREEN_FLAKY hold, what it validated: `{tip, parent,
  changes: [{pr, head}], prefixes, claim, run, validation}`. `accepted`: `{by, reason, at}` once
  `accept-flaky` accepted it, until a run acts on the ruling.
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
| `net_timeout_secs` | `0` | Seconds one network command may run before it is killed and fails; `0` means 300. An older binary warns about the field and ignores it. |
| `precheck` | absent | `{command, timeout_secs, background, background_timeout_secs, covers}`: the precheck of submitted heads (`timeout_secs` absent or `0`: 300), the command a background worker runs on every queued head (`background_timeout_secs` absent or `0`: 1800), and the validation steps the two reproduce. Left out of the file while unset; an older binary warns about the field and ignores it, and one that knows `precheck` but not `background` or `covers` refuses the configuration. See "The precheck". |
| `combine` | `rebase` | `rebase` or `merge`. |
| `max_batch` | `4` | The maximum number of changes per batch. |
| `depth` | `2` | The initial window and the culprit-search probe width. |
| `max_depth` | `0` | The largest adaptive window; `0` means fixed at `depth`. |
| `value_ordering` | `false` | Value-ordered scheduling: a waiting batch whose historical green rate (per change, from the decision log) beats the deepest in-flight batch's aborts it for its slot; the aborted batch is replanned later in the run (a `value-abort` event). |
| `disable_disjoint_root` | `false` | Turn off the disjoint second root: with a window of at least 2, one slot validates a batch built on the base itself, disjoint (no textual conflict, no dependency) from every batch in flight, so a red stack cannot take the whole window down. |
| `disjoint_root_min_red_percent` | `0` | Only start a disjoint second root when the red share of the last 20 decided validations in the decision records is at least this percent; `0` (the default) always starts one when it fits. Set it with `init --disjoint-root-min-red-percent`. |
| `roots` | `1` | How many base-rooted batches may validate at once, each disjoint from everything in flight. `1` is today's behaviour (one disjoint root in the window's last slot); at least the window (`depth`) means every slot may be a root. Set it with `init --roots`. |
| `keep_stale_roots` | `false` | Keep a rooted batch whose base moved instead of cancelling it: a green one is requeued at the front to be rebuilt on the new base (its history says `green at <base>`), a red one is decided as usual. Set it with `init --keep-stale-roots`. |
| `stack_min_pass_percent` | `0` | Build on the stack top only when the recent pass rate (green share of the last 20 decided validations) is at least this percent; below it, build a root on the base when `roots` allows. `0` always stacks. Set it with `init --stack-min-pass-percent`. |
| `batch_min_pass_percent` | `0` | Build batches wider than one change only when the recent pass rate is at least this percent; below it, batches are one change wide whatever `max_batch` says. `0` always allows width. Set it with `init --batch-min-pass-percent`. |
| `union_paths` | `[]` | Attribute patterns merged with Git's union driver (see "Union merges"). |
| `priority` | absent | `{label, ci_paths}`: the priority label (absent or empty: `mq-priority`; `-`: off) and the CI path patterns for label suggestions. Left out of the file while both are defaults. See "Priority changes". |
| `regenerate` | empty | `{paths, command, when}`; see "Generated files". |
| `generated_paths` | `[]` | Top-level spelling of `regenerate.paths` (globs naming the derived files); merged into `regenerate.paths` on load. Left out of the file while empty. |
| `exact_sha` | `false` | The exact-commit rule. |
| `exact_merge_allowed` | `false` | Operator-asserted exception: exact merge commits may land even when the base branch's rules require linear history. Set only while an owner-granted ruleset exception is in place (init `--exact-merge-allowed`). Left out of the file while false. |
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
| `notify_mode` | `both` | `both`, `hooks-only` or `notify-only`: how event hooks and built-in notify divide reports (left out of the file while `both`); see "Event hooks". Set with `init --notify-mode`, overridable per change with `submit --notify-mode`. |
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
- `empty_red`: `could-not-run` treats a red run that recorded no test result at all as a
  could-not-run refusal (see "A red with no recorded test result"); absent or `red` (the
  default) keeps it as a red verdict. A binary older than this field refuses a config that
  has it.
- `fail_fast`: `true` sets `$MQ_FAIL_FAST=1` for candidate validations (see "Fail-fast runs");
  absent (the default) means off. A binary older than this field refuses a config that has it.
- `disable_early_red`: `true` turns off the early-red watch (see "Early red (streamed
  verdicts)"); absent (the default) means the watch is on. A binary older than this field
  refuses a config that has it.
- `disable_early_red`: `true` turns off the early-red watch (see "Early red (streamed
  verdicts)"); absent (the default) means the watch is on. A binary older than this field
  refuses a config that has it.
- `confirm_reruns`: `true` turns same-commit confirmation on: red batches are searched and a
  blamed failure must be seen twice (see "Keeping the base green"). Absent (the default) means
  off: the first red is the verdict. A binary older than this field refuses a config that has it.
- `split_red_batches`: `true`, with `confirm_reruns`, splits a red batch of several changes into
  single-change batches instead of searching it, so confirmation costs only a rerun of a single
  change's failed tests at its own commit. Absent (the default) means off. A binary older than
  this field refuses a config that has it.
- `suspect_flake_reruns`: `true`, with `confirm_reruns` off, holds a single change whose red run
  failed only on suspect tests and reruns them beside the run (see "Suspect flakes"). Absent (the
  default) means off. A binary older than this field refuses a config that has it.
- `suspect_flake_timeout_secs`: wall limit of one suspect-flake rerun (absent or `0`: `1800`).

**`donors`** fields (only read when `validator.offload` is not off; absent means the defaults):

- `lease_secs`: how long a lease lasts without a renewal (default `120`; a donor renews every
  quarter of it).
- `max_bundle_bytes`: the largest candidate bundle served to a donor (default 256 MiB).
- `max_jobs_per_donor`: the most leases one donor may hold (default `0`: what the donor asks for).
- `claim_timeout_secs`: in `decide` mode, how long a job may wait with no donor holding it
  before the run stops with an error (default `900`). Nothing is ejected.
- `max_could_not_run`: could-not-run receipts one job may collect before it is cancelled, and
  one donor may return for it before it is left out (absent or `0`: `3`).
- `alert_after_give_backs`: consecutive give-backs (releases or declines) with the same
  reason from one donor that raise an `ERROR:` line in `donor status` and the serve
  heartbeat's `last error` (absent or `0`: `3`). An accepted receipt from the donor resets
  its run.
- `host_fault_escalate_after`: consecutive probe-pass/launch-refuse cycles after which a
  donor's host fault escalates (absent or `0`: `5`); see "Could-not-run is not a host fault
  by itself" below.

**Could-not-run is not a host fault by itself.** A validator prints
`MQ_COULD_NOT_RUN_SCOPE: host <reason>` with its exit-75 refusal when it cannot run at all
on its host (a wedged worktree allocator, say); `job <reason>` (or no line) is
candidate-specific. Either way the refusal alone never faults the host: the job is queued
again, the receipt is counted (`max_could_not_run` bounds one job, and one donor is left
out of a job it refused that many times), and the donor probes its validator right after
with `MQ_PROBE=1` (admission checks only, exit 0 = ready). A passing probe means the host
is healthy — the refusal was candidate-specific — and the donor keeps leasing. Only a
failing probe records a host fault: the donor is then leased nothing, and `donor status`
plus the serve heartbeat show `donor <name> faulted since <t>: <reason>; last probe <t>
<result>`. An admission refusal that says the validate lock or box is busy (slots full, a
queue position, lease contention) is never a host fault and is never probed. While faulted
the donor re-probes on its own schedule — after 60 s, doubling to at most 5 min — whether
or not a host-pause file exists, counting one probe cycle per probe, so a fault never sits
at zero probes forever. A passing re-probe (or an admitted validation) clears the fault.
Operators can clear one without leasing anything with `merge-queue donor clear-fault
--donor <name> --probe-passed` (run it where the queue state lives, after the donor's
probe passed); unlike `donor claim --probe-passed` it leases no job. While every eligible
donor is faulted, a decide-mode run keeps waiting instead of spending its claim timeout;
with any eligible donor known but healthy, the normal claim timeout applies even if it
never claims.

A probe pass followed by a launch refusal whose own probe then fails is a flap cycle,
not a recovery: the donor's probe back-off keeps doubling (60 s to 5 min) across such
cycles instead of resetting, and only an admitted validation resets it. After `host_fault_escalate_after` consecutive
cycles (5 by default) the fault escalates: `donor status` and the serve heartbeat raise an
`ERROR:` for it, and the donor stays paused until an admitted validation clears it — a
probe pass then only arms one trial claim, it no longer clears the fault.
- `compare_only`: donor names that only ever compare in `decide` mode. Each published job
  gets a duplicate only they may claim; its receipt is recorded as a comparison (`comparison`
  on both jobs, counted by `donor status`) and never decides the batch. Deciding donors never
  claim the duplicates. In `compare` mode every donor already only compares, so the list has
  no effect there.

**`conflict_prior`** fields (absent means off):

- `footprints`: path in the repository of a test-footprints JSON
  (`{"<test id>": ["<path>", ...]}`, read at the plan's base; `--conflict-footprints`).
  Two changes sharing an affected test (a test whose footprint touches both changes'
  files) are marked suspect in the plan's `prior_conflicts` and are merge-tree probed
  even when their files are disjoint. The prior never decides anything: the probe
  remains the only conflict verdict, and the planner's mechanism/overlap edges are
  advisory input of the same kind, not a second verdict.

**`peripheral`** fields (the peripheral lane; absent means off):

- `command`: a shell command run in a checkout of each change head with `$MQ_BASE`,
  `$MQ_HEAD` and `$MQ_PR`. A first output line of exactly `peripheral` marks the change
  peripheral; anything else, an error, or a timeout means binary-touching (fail closed).
  The consumer's rule lives in this command: peripheral only when the change touches no
  input of the product binary build and no test or fixture the full selection would run
  differently. Empty (the default) disables the lane.
- `timeout_secs`: seconds one call may run (default `30`).
- `max_batch`: largest batch of only peripheral changes (absent or `0`: twice `max_batch`).

Answers are cached in `probe-cache.json` per (hook, base, head): an unchanged pair is never
re-asked, a changed head or base always is, and the plan and decision records say whether an
answer came from the `hook` or the `cache`. Peripheral changes never share a batch with
binary-touching ones, batch up to the wider cap, and validate with `$MQ_PERIPHERAL=1` set
for the select command and the runner, so the selection can be the focused one.

**`land`** fields:

- `driver`: `push` or `command` (see "Land drivers"). Any other value makes `run` refuse to start
  unless it is a `--shadow` or `--dry-run` run.
- `command`: the shell template for the `command` driver.
- `allow_batches`: for the `command` driver, allow batches of several changes (default `false`,
  which makes every batch one change).
- `push_max_retries`: extra attempts for a `push` landing after a transient remote-side
  rejection (default `0`, meaning 2 extra attempts, three tries in all).
- `push_retry_backoff_secs`: base backoff in seconds between push retries, doubled per retry
  (default `0`, meaning 40, so the default retries span about two minutes).

## Validators

Every validator takes a selection and returns per-test results. Both kinds share the selection
policy: `select` runs in a checkout of the candidate with `$MQ_BASE` (the candidate's parent) and
`$MQ_HEAD` set, and prints test IDs, one per line (only lines starting with `id_prefix` count; a
line `all` selects everything). Without `select`, every validation is complete. Every validation
runs in the candidate checkout, under the `admission` prefix, with the `timeout_secs` limit.

**Tests beyond the complete suite.** A runner's complete suite need not hold every test: some
are too slow or too costly to run on every candidate, and only matter when a change touches what
they test. A `select` that prints `all` and also test IDs asks for the complete suite plus those
tests. The runner gets `$MQ_SELECTION=all` and the IDs, space separated, in `$MQ_EXTRA_TESTS`
(set only when there are some). A `dagrun` runner does not run them, so with it they count as
missing: they are for the `command` kind. A failing one makes the
validation red like any test, and the culprit search that follows reruns it as a selected test
(`$MQ_SELECTION=tests`). One the runner never reports counts as missing, never as a pass. The
`validate` event names them (`all + super.a, super.b`), and so does the validation's
`record.json` (`extra`). A remainder that runs the complete suite after an ejection runs the
batch's extra tests too.

**`command`.** `run` is a shell template (`sh -c`) that reads the selection from:

- `$MQ_SELECTION`: `all` or `tests`;
- `$MQ_TESTS`: the IDs, space separated (empty for `all`);
- `$MQ_TESTS_FILE`: the IDs, one per line;
- `$MQ_EXTRA_TESTS`: with `all`, the tests to run beyond the complete suite, space separated
  (see "Tests beyond the complete suite"; unset when there are none);

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
tests, and starts it again in the same run after a backoff (`refusal_backoff_secs` doubled per
attempt, up to `refusal_max_retries` extra attempts). The retry is a validation like any other,
beside the batches still validating: the batches built on the refused one stay, since a refusal
says nothing about the candidate, and the run goes on taking requests and landing while it waits.
A batch refused behind the head is retried at once, not when it reaches the head, and if its
retries run out there it gets them afresh when it becomes the head, since by then its refusal may
be as old as the head's whole validation. If the batch is still refused after its last attempt,
it is deferred once it is decided: every
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

**An unparseable results row is could-not-run too.** If any row of a `command` validator's
results file cannot be parsed (bad JSON, an unknown outcome, or an invalid test id), that
validation is could-not-run with host scope, exactly like a refusal above: it is not an error
that stops the run, and no row of the file — parsed or not — is a verdict for or against the
change. The detail names the file, the line number and the parse error (cut to about 300
bytes), for example `could not run: bad line in .../results.jsonl line 2: ...`; the rows that
did parse stay in the validation record for diagnosis. The usual refusal retries, backoff and
deferral bound apply, so an adapter that deterministically writes a bad row parks its change
after the bound with a notification instead of re-validating the same batch forever. A donor
receipt carries it as a host-scope could-not-run (`MQ_COULD_NOT_RUN_SCOPE: host ...`).

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

**A red with no recorded test result.** A `command` validation that writes nothing to
`$MQ_RESULTS` and prints no verdict lines recorded no test result: nothing ran, so its exit
status is the only signal and its failed set is empty. What that red means is set by
`validator.empty_red` (`init --empty-red`):

- `could-not-run`: the red is treated as a could-not-run refusal on the existing bounded
  path (see "Admission refusals" and "Bounded retry"). It costs the change no strike, is
  requeued at the same head, and counts against `validator.max_deferrals`; a change that
  really breaks the harness before any test still escalates to its owner at the bound and
  is never retried forever. A consuming queue whose harness can crash before any test
  starts sets this.
- `red` (the default, also when the field is absent): existing queues keep their
  behaviour — the red is the verdict — except that an ejection with an empty failed set is
  never named as an empty failed-test list. Its reason, event and report say
  `red with no recorded test result` instead.

Either way, a red that did record results (a named failure, or passes beside it) ejects
normally under both settings, and a culprit-search probe that is an empty red observes
nothing: it is inconclusive and can name no culprit, so bisection within a multi-change
batch is unaffected by it.

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

**Same-commit reruns.** Only with same-commit confirmation on (`validator.confirm_reruns`): off,
the queue never validates a commit again to rerun its failures and never sets `$MQ_RERUN_FROM`.
A runner may write an opaque handle for its run (a run ID, a log path)
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
a red single-change batch costs exactly one targeted rerun (none with confirmation off).

**Deterministic checks.** Some checks depend only on the tree: a lint, a path audit, a grep. A
rerun of the same commit cannot change their answer, so a confirming rerun only costs time. A
runner says so on a red run with one marker line naming the steps:

- `RERUN-EXEMPT: step1,step2` with a red exit (not `0`, `6` or `75`): those steps are
  deterministic checks.

When every failure the run attributed belongs to a named step, the queue counts that red as
confirmed on this one sighting. A red single-change batch is ejected without a rerun; in a larger
batch, a probe that fails this way confirms its prefix at once. The ejection reason says `seen
once: a deterministic check, which a rerun of the same tree cannot change`, where it otherwise
says `seen twice`. With same-commit confirmation on, a run with any failure outside the named
steps, or with no attributed failure, still needs its failure seen twice; off, every red counts on
its first sighting anyway. The marker with another exit status means nothing. A donor's
receipt carries it in its log tail.

**Fail-fast runs.** With `validator.fail_fast` (`init --fail-fast`), the queue sets
`$MQ_FAIL_FAST=1` for every validation it runs on this host except the complete run on the base
(`widen`), whose record should name every failure. A runner that honors it stops at the first red
step and reports each step it skipped as `skip`, never `pass`: a skipped test was not seen to
pass, so a remainder after an ejection reruns it. A runner that ignores the variable is still
correct, only slower to report red.


**Early red (streamed verdicts).** By default the queue watches a running validation and stops
it at its first deterministic failure, so a red candidate releases its slot and the batches built
on it at once instead of holding them until the runner exits. `validator.disable_early_red: true`
turns the watch off. The adapter contract is:

- The queue sets `$MQ_EARLY_RED=1` in the runner's environment when the watch is on (for a
  `dagrun` runner, in dagrun's environment). A runner that ignores the variable is unchanged.
- To opt in, a `command` runner appends each verdict to `$MQ_RESULTS` as the test finishes and
  flushes the file (a line written whole; a partial line is read on the next poll). A `dagrun`
  runner opts in through dagrun itself: each `step_end` it appends to `journal.jsonl`, and its
  verdict lines (`[step] ✗ FAIL ...`) in the combined output count too.
- Deterministic steps are declared with the same `RERUN-EXEMPT: step1,step2` line as above,
  printed (and flushed) before or with the first failure it covers, not only at exit. An adapter
  may also print `MQ_EARLY_RED: step1` at the first deterministic red; the queue needs only the
  declaration and the failure.
- A failure counts as deterministic when it is a completed `fail`/`error` verdict whose step is
  already declared. A timeout or no-verdict failure never stops a run early, and neither does a
  failure in any other step.
- When one lands, the queue stops the runner's whole process group (TERM, then kill after a
  short grace, as for a cancel), records the run as red with `early_red: true`, and decides the
  batch on the verdicts seen so far; tests never reported carry no outcome. An adapter that
  exits 1 by itself at the first deterministic red, with `$MQ_RESULTS` and the declaration
  already written, gets the same decision without being stopped. Publication and other post-run
  work still happen after the verdict, off the slot.
- The watch composes with fail-fast: with both on, `$MQ_FAIL_FAST=1` asks the runner to stop
  inside its own step while the watch stops the whole run; whichever acts first reports red.
- With `validator.offload: decide`, receipts arrive whole, so there is no early red from donors;
  the watch applies to validations this host runs.


**Early red (streamed verdicts).** By default the queue watches a running validation and stops
it at its first deterministic failure, so a red candidate releases its slot and the batches built
on it at once instead of holding them until the runner exits. `validator.disable_early_red: true`
turns the watch off. The adapter contract is:

- The queue sets `$MQ_EARLY_RED=1` in the runner's environment when the watch is on (for a
  `dagrun` runner, in dagrun's environment). A runner that ignores the variable is unchanged.
- To opt in, a `command` runner appends each verdict to `$MQ_RESULTS` as the test finishes and
  flushes the file (a line written whole; a partial line is read on the next poll). A `dagrun`
  runner opts in through dagrun itself: each `step_end` it appends to `journal.jsonl`, and its
  verdict lines (`[step] ✗ FAIL ...`) in the combined output count too.
- Deterministic steps are declared with the same `RERUN-EXEMPT: step1,step2` line as above,
  printed (and flushed) before or with the first failure it covers, not only at exit. An adapter
  may also print `MQ_EARLY_RED: step1` at the first deterministic red; the queue needs only the
  declaration and the failure.
- A failure counts as deterministic when it is a completed `fail`/`error` verdict whose step is
  already declared. A timeout or no-verdict failure never stops a run early, and neither does a
  failure in any other step.
- When one lands, the queue stops the runner's whole process group (TERM, then kill after a
  short grace, as for a cancel), records the run as red with `early_red: true`, and decides the
  batch on the verdicts seen so far; tests never reported carry no outcome. An adapter that
  exits 1 by itself at the first deterministic red, with `$MQ_RESULTS` and the declaration
  already written, gets the same decision without being stopped. Publication and other post-run
  work still happen after the verdict, off the slot.
- The watch composes with fail-fast: with both on, `$MQ_FAIL_FAST=1` asks the runner to stop
  inside its own step while the watch stops the whole run; whichever acts first reports red.
- With `validator.offload: decide`, receipts arrive whole, so there is no early red from donors;
  the watch applies to validations this host runs.

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
batches built on top of it are discarded. Two rulings end a hold: `merge-queue retry PR` returns
the change to the queue for a new validation, and `merge-queue accept-flaky` accepts a green
batch's GREEN_FLAKY passes and lands the very commit that was validated if the base has not
moved (the hold records it in `held_candidate`). When the hold ends without a ruling, the change
is re-queued for a fresh validation (which holds it again if the result is the same). A
GREEN_FLAKY result whose retried tests are all on the known-flaky list is never held: the list is
a standing ruling, and the batch lands (see `merge-queue known-flaky`). A donor's receipt carries
the markers in its log tail.

With `validator.green_flaky: "land"` (or `--green-flaky land` at `init`), a GREEN_FLAKY result
whose tests are not all on the list lands the same way, by policy instead of by ruling: no hold,
no `accept-flaky` needed. The config is the standing ruling — recorded as
`validator.green_flaky=land` in `rulings.jsonl` (ruling `green-flaky-policy`) — and the retried
tests are recorded in the landing record, the item-history `landed` entry (`; flaky: <ids>`), the
run's `report.md` and the `green-flaky-landed` event. A flaky result still never passes silently:
it lands, and it is recorded. INFRA-BLOCKED holds whatever this setting says.

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
  same culprit search, flaky detection and obligations as a local result. A donor named in
  `donors.compare_only` never gives that verdict: it validates a duplicate of the candidate
  and its receipt is only compared with the deciding one. The queue host keeps
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

JSON replies (`donor info`, `claim`, `renew`, `report`) print exactly one JSON value on
stdout, with any config warning carried in a `warnings` field of that value (and once on
stderr): nothing may precede the JSON, because some routes merge the remote's stderr into
stdout. The donor parses the last JSON line of a reply, tolerating leading noise lines
(warnings, an exec route's notes) and logs skipped noise once.

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

**Stale jobs.** A published candidate is built on the base at publish time. Once the base
moves to a commit the candidate does not contain, the job can never land as published: the
queue cancels it as `stale: the base moved to <base>, which its candidate does not contain`
when a run settles its jobs, and a claim sweeps stale jobs before leasing anything, so a
donor never spends its setup on one. Stale is not a donor failure and counts toward no
could-not-run bound.

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
  (non-force) push. A non-fast-forward rejection means the base moved: the batch is rebuilt on
  the new base and validated again. Push failures are classified once, as transient, permanent
  or unknown. A transient remote-side rejection (a remote fault such as a `commit_refs`
  failure, a 5xx, a dropped connection, or an SSH/proxy transport error —
  `kex_exchange_identification`, `banner exchange`, `Connection to UNKNOWN port`,
  `ssh: connect to host`, or `Could not read from remote repository` after one of those) is
  retried with bounded backoff (`land.push_max_retries` extra attempts,
  `land.push_retry_backoff_secs` doubled per retry; with the defaults, three tries span about
  two minutes), and the base is checked after every attempt. After the push, the queue fetches
  the base and requires it to equal the candidate. For a local repository (`--remote ''`) the
  base branch is moved with a compare-and-swap `git update-ref`.

  If a run stops anyway (a push still failing after its retries, or another stop with batches
  in flight), its green batch validations are kept in `kept-validations.json` instead of being
  discarded. The next run lands a kept batch first, without validating again, while the base
  has not moved under it: every change is still queued at the validated head, the candidate
  rebuilt on the current base has exactly the validated tree, and the selection is the
  validated one. A kept batch that no longer matches is dropped and validated as usual.

  A stop decision is recorded at once: the run writes its `stopped` event before it waits on
  anything, cancels or detaches the validations still in flight (a green that finishes later
  is still kept, above), and never re-leases a donor job that was refused after the decision.
  While it is parked that way, `status` and the serve heartbeat show
  `stopping: <reason>; waiting for <batches> to return`.

  A permanent push failure is not a stop: when the remote output says `GH013` or
  `Repository rule violations` (the classifier's permanent family), the push is tried once,
  the candidate's changes are ejected as `push-rule-violation` quoting the violated rule
  (for example `This branch must not contain merge commits`), their owners are notified as
  for any ejection, batches behind them are rebuilt on the base, and the run continues. It
  is never retried, in that run or a later one.
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
   at once. With a window of at least 2, one slot instead takes a
   **disjoint root**: a later batch that shares no change, no textual conflict and no dependency
   with anything in flight, built on the base itself (a `disjoint-root` event). It records the
   batches in flight when it starts, the ones it jumped over. It is not cancelled when the stack
   is discarded, and it is decided as soon as every batch it jumped over is decided, without
   waiting for the rest of the queue: if none of them landed, its parent is still the base and it
   lands on its own validation. The moment any batch ahead of it lands, it is cancelled instead
   (a `disjoint-root-cancelled` event), its slot is freed at once, and its changes go back to the
   front of the queue to be rebuilt on the new base. A disjoint root never lands on a base other
   than the commit it was built on. Once it is decidable and nothing it jumped over landed (a
   red head was ejected, say), it is promoted to an ordinary stack head (a
   `disjoint-root-promoted` event): batches refilling the window build on its tip, so when it
   lands they land straight after it instead of going stale on the base. `disable_disjoint_root` turns it off, and
   `disjoint_root_min_red_percent` only starts one when the recent red rate makes a red head likely.
   Green batches land in order, as soon as every batch ahead of them has landed, and the
   next batch starts at once on top of the last one in flight.
2. **AIMD.** The window starts at `depth`, grows by one per landed batch up to `max_depth`, and
   halves per red batch it searches or decides alone whose every failure timed out or reached no
   verdict (as the validator shapes its result rows), never below one: only such a red says the
   host may be overloaded. Any other red names a culprit and says nothing against the batches
   behind it, so it leaves the window as it is, as a red batch that is split (below) does. With
   `max_depth` 0 it stays at `depth`. The report's
   `windows` field records the window each time a batch started.
3. **A red head** cancels everything behind it: those candidates were built on its tree. Their
   validations are killed, reported as `cancelled` (never as red), and their batches go back to the
   front of the queue, to be rebuilt once the red batch is resolved. **A red batch behind the
   head** is decided in order too, once every batch ahead of it is: until then the head's changes
   could be the cause. But the batches built on its tree cannot land whatever they are, so they
   are cancelled the moment its result comes (a `red-behind-head` event), and the red batch gives
   up its place in the window. Their changes, and requests recorded meanwhile, are built beneath
   it, on the last batch in flight that is not red, and validate in the freed slots at once. When
   the red batch's turn comes, the batches behind it are kept, since none was built on it.
4. **Split, or culprit search.** With same-commit confirmation off (the default), a red batch of
   several changes is split instead of searched: each change goes back, in order, to the front of
   the queue as a batch of its own, and stays alone in every later plan of the run, including the
   plans made for requests taken in during the run. Each is then decided by its own validation,
   built on the one before it. A split ejects nothing, halves nothing and starts no probe, so the
   singles validate side by side and the run keeps taking in requests while they do. With
   confirmation on, the queue searches the red batch's prefixes (`parent + first i changes`) for
   the first one that fails, running **only the failed tests**. Each round probes up to `depth`
   prefixes at once, evenly spaced (a W-ary search, not halves). The bare parent is probed until it
   is known green. A prefix search respects stacked changes and also catches a change that fails
   only in combination with the ones before it.
5. **Seen twice, or once.** With confirmation off, one sighting confirms. A red change on its own
   is ejected after its own red validation and, unless `trust_base`, one probe of the bare parent
   on the failed tests; a parent that fails them stops the queue (exit `3`) instead. The reason
   says `seen once: same-commit confirmation is off (validator.confirm_reruns)`. With confirmation
   on, every blaming conclusion needs its failure observed twice: a culprit's prefix
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

## The k-wide baseline

The default speculation is a stack: each batch builds on the previous one, so one red batch
cancels everything built on it. When most batches are red, that wastes the window. The k-wide
baseline turns the window into `k` independent single-change validations on the current base:
the first green lands, the rest go stale and are rebuilt on the new base. Stacking and wide
batches come back only when the recent pass rate earns them:

```json
"max_batch": 4, "depth": 6, "max_depth": 6,
"roots": 6, "keep_stale_roots": true,
"stack_min_pass_percent": 80, "batch_min_pass_percent": 80
```

With `roots` at least the window, every slot may hold a base-rooted batch, each disjoint from
everything in flight. `keep_stale_roots` lets a stale root finish instead of cancelling it: a
green one is requeued at the front of the queue (ahead of never-validated changes, which have
no pass prediction) with `green at <base>` in its history, and a red single change is ejected
as the culprit it is, since the base it ran on was green. Below an 80% recent pass rate nothing
stacks and every batch is one change wide; at or above it, batches widen to `max_batch` and
build on the stack top as usual. Every field absent is today's behaviour exactly.

One exception below the gate: a candidate that was green on the immediately previous base and
went stale only because unrelated changes landed — its files share no path with what landed
since its green, and merging its green candidate onto the current stack head is clean — is
stacked on that head anyway, so it lands in the same round instead of rebuilding as another
disjoint root on the new base. The run records a `stale-green-stacked` event (the candidate,
the previous green base, and the landed changes it was checked against). Fresh candidates
still wait for the gate, a candidate whose paths overlap what landed or whose merge conflicts
rebuilds as today, and a stacked validation that comes back red is handled as an ordinary
red, with no strike beyond the normal one.

## The precheck

A change that fails a cheap, deterministic check (a formatter check, a documentation build)
sinks every batch it joins, and with speculation the batches built on that one too, for the
length of a validation. The precheck runs such checks once, on the submitted head alone, before
the change is queued, so that head never joins a batch.

`precheck.command` is a shell command. It runs at the root of a checkout of the submitted head
that the queue keeps for it (`scratch/precheck/`, switched with a forced checkout plus
`git clean -ffd`, so ignored build caches are kept), with these variables set:

| Variable | Value |
|---|---|
| `MQ_PR` | The change number. |
| `MQ_HEAD` | The head commit being checked. |
| `MQ_HEAD_REF` | The head branch or ref it was submitted with. |
| `MQ_BASE` | The base commit the queue fetched with it. |

How it ends decides what happens:

- **Exit 0** passes. The change is queued, and its `submitted` event says how long the check took.
- **Any other exit** fails. The change is **ejected at submission**: its history holds a
  `submitted` event, then an `ejected` event that begins `refused at submission:` and quotes the
  last 40 lines of the command's output (paths inside the checkout are shortened to paths in the
  repository; the whole output, up to 16 KiB, is kept with the verdict). `submit` exits non-zero
  and prints the same text. A change submitted with `--notify NAME` is notified at once, even
  while a run holds the queue, rather than when that run ends. `sync` treats the head as settled
  and does not submit it again until the head moves.
- **A check that could not run** says nothing about the change, so the change is queued anyway
  and its `submitted` event says why the check was skipped. That covers exit 75 (a temporary
  refusal), 126 and 127 (a command that cannot execute or is not found), death by a signal, a
  command that cannot be started or a checkout that cannot be made, and a command still running
  at `precheck.timeout_secs` (its process group is killed).

A pass or a failure is kept per head and command under `precheck/verdicts/`, so submitting the
same head again gets the same answer without running the command; a check that could not run is
tried again the next time. A new head is a new check. `merge-queue submit --skip-precheck`
queues a head without the check when a refusal is wrong; its history says so, and the skip is
kept as that head's verdict.

The precheck runs where a submission becomes a queued change (a `submit` that takes the writer
lock, a recorded submission when it is applied, and each change `sync` submits), and again when
a run builds a batch, for every change in it. The second check answers at once from the kept
verdict of a head checked at submission. It catches a change queued before the precheck was
configured: if that change fails, it is ejected as `precheck` and the batch is built without
it. Keep the command to checks that take seconds, or a minute at most on
a warm build cache: it runs under the writer lock, so a run taking in recorded submissions or
building a batch waits for it. Slower checks belong in `precheck.background`.

### Slow checks in the background

`precheck.background` is a second shell command, for checks that take minutes (a documentation
build, a linter that compiles the changed crates). It is never waited for. A worker thread in
the process that runs the queue (the `serve` daemon, or `merge-queue run`) runs it on every
queued head, one head at a time, in the order of the run's plan, in a checkout of its own
(`scratch/precheck-background/`), with the same variables and exit statuses as
`precheck.command` and a deadline of `precheck.background_timeout_secs`. Its verdicts are kept
under `precheck/verdicts/` like the others.

- When a batch is built, a change whose kept background verdict is a failure is ejected as
  `precheck`, with the command's output, and the batch is built without it. A change not checked
  yet joins the batch, and its verdict counts when it comes.
- A red batch whose every failed test is a step listed in `precheck.covers` (the validation steps
  the two commands reproduce, such as a formatter step and a documentation step), and in which
  some change has a kept precheck failure, is explained by it: those changes are ejected, and the
  rest of the batch goes back as one batch, without a culprit search or a split. The run's event
  log says so (`precheck-culprit`).
- A background check that could not run is tried again after 30 minutes. The worker kills its
  check when the process stops or exits.

### Suspect flakes

With same-commit confirmation off, the first red is the verdict, and a test that only timed out
under load costs its change an ejection and a resubmission. `validator.suspect_flake_reruns`
gives such a red one more look, without holding the run for it.

- A runner may say how a failed test failed, with `"shape"` on its result row: `timeout` (it ran
  out of time: a wall or CPU limit, an exit 124) or `no-verdict` (the run reached no verdict).
  A failure without a shape is an assertion.
- When a single-change batch is red, the run attributed its failures, and every failed test has
  one of those shapes, the change is held (`SUSPECT-FLAKE`) instead of ejected: the batches built
  on it are rebuilt without it, and the run goes on. The window is not halved.
- A worker thread in the process that runs the queue validates just those tests again at the red
  run's exact commit, in a checkout of its own (`scratch/suspect/`), as a plain validation
  (`$MQ_SELECTION=tests`), never a rerun composed from the red run, under
  `validator.suspect_flake_timeout_secs`. One rerun at a time.
- The run acts on the verdict at its next batch boundary: if the tests pass, the change goes back
  into the plan, its history recording them as GREEN_FLAKY; if they fail again, the change is
  ejected, seen twice; if the rerun could not run, the change goes back into the plan and its next
  validation decides. A one-shot `run` waits for its reruns before it ends.
- Once per head (`suspect_reruns` on the item): a second suspect red of the same head is blamed as
  any red is. A hold whose rerun never reports (its process stopped) expires 15 minutes past the
  rerun's limit and re-queues the change; a run that starts hands the reruns of changes still held
  to its own worker again.

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

Two other files sit beside the day files; `stats` and the pruning read only the day files.
`decisions/skips.jsonl` has one line per change left out of its batch. `decisions/rulings.jsonl`
has one line per ruling on a GREEN_FLAKY batch (schema `merge-queue-ruling/v1`): `ruling`
(`accept-flaky`, or `known-flaky` for the list's standing ruling), `prs` and their `heads`, the
validated `commit` and its `base`, the retried tests (`flaky`), `by`, `reason`, `ruled_at`,
`outcome` (`landed`, `requeued`, or `stopped` when the landing left the base to check by hand)
with its `detail`, `validated_in` (run and validation), and the `run` that acted on it with `at`.

## The per-PR signal log

Beside the decision log, the queue keeps one append-only JSONL per change in
`signals/<pr>.jsonl` (schema `merge-queue-signal/v1`): the history a
success-probability model reads. Kinds: `head` (new head, parent head, reason),
`focused-tests` (actor, head, test IDs, outcomes), `review` (reviewer, model
family, verdict, head), `validated` (batch members, candidate, verdict, failing
tests, attribution `culprit`/`cleared`/`undetermined`), and `full-validate`
(ledger run ID and verdict for an exact head or an ancestor). The queue writes
`head` on every submit and `validated` on every run; external producers use:

```sh
merge-queue signal add 123 --kind focused-tests --json '{"actor":"dev","head":"abc","tests":["a"],"outcome":"pass"}'
merge-queue signal list 123 [--json]
merge-queue signal backfill   # synthesize head+validated from decisions and items; idempotent
merge-queue signal export --to DIR   # append not-yet-exported lines to DIR/<pr>.jsonl
```

Only the queue binary writes the files. Backfill never invents focused tests or
reviews. Decision records carry `predicted_pass`: the base-rate model - each
change's first component's historical green rate over earlier decision records
(global rate when a component has none), recorded once at least 10 decided
validations exist, otherwise `null`.

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

**Network commands are bounded.** Every network command the queue runs (`git fetch`,
`ls-remote` and `push`, through `--net-wrapper` when one is set, and every `gh` call) gets
`net_timeout_secs` (default 300 seconds). Past that its whole process group is killed and the
command fails with `... timed out after Ns (its process group was killed)`. A hung transport, or
a credential helper waiting for budget, then costs a retry at the next cycle instead of holding
a run, a landing or the daemon's loop. A planning fetch that times out is not retried one ref at
a time, because a hung transport is not a missing head. A landing push that reports a failure
while the base already is the pushed tip has landed: the remote took it and only the report was
lost, and the run says so in a `push` event.

## Generated files

Some repositories commit files generated from sources, such as a validation graph compiled to
JSON or a static table compiled to code. Two changes that each regenerated such a file conflict
on it, even when their sources combine cleanly. If those files cause most of a backlog's rebases,
list them with `--regen-path` and give the command that rebuilds them with `--regen-command`.
The same set can be declared in `config.json` as the top-level `generated_paths` list of globs;
it is merged into `regenerate.paths` when the configuration loads. `merge-queue compose --check <PR>`
is a read-only dry run against the current base: it lists which of the change's conflicts would be
auto-regenerated and which need a human (every conflict of an `--exact` change needs a human, because
an exact change is never rewritten). Each auto-regenerated conflict is recorded as a
`regenerated-conflict` run event naming the paths.

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
  `MQ_REGEN_PATHS` (the configured generated patterns, space-separated), `MQ_CONFLICTED` (the
  generated paths that conflicted and were settled on the target's side, space-separated; empty
  when nothing conflicted), `MQ_HEAD` (the change's head), `MQ_PARENT` (the commit the change was
  combined onto) and `MQ_CHANGE_BASE` (the commit the change's own edits start from). For a change
  stacked on another change whose copy `MQ_PARENT` already holds (landed, in the batch beneath,
  or earlier in this batch), `MQ_CHANGE_BASE` is that change's head; otherwise it is the merge
  base of `MQ_PARENT` and `MQ_HEAD`. All three are empty when the command runs on a tree with no
  change of its own, as when the queue checks the generator on the tree before a change. The
  command must rewrite every generated file deterministically from the checkout's sources and
  touch nothing else: for the same sources it must produce byte-identical output on every run,
  and the queue refuses any run that changes a file outside the generated set. A minimal correct
  example that rebuilds `gen/all.txt` from its sources is `mkdir -p gen && cat $(ls src/* | sort) > gen/all.txt`; a full example with paths and sources of your own follows the same shape — run in
  the candidate checkout, read `MQ_CONFLICTED` if you need to scope work, and never write outside
  the generated paths. A stdout line `merge-queue-note: TEXT` records what the command did (for
  example "renumbered #12's revision 40 to 41"): the queue adds it to the `regenerate` event, to
  the regeneration in the run report and, once per run, to the change's history as
  `regenerate-note`.
- **Partly hand-written files.** A file that holds a table the command keeps in step, beside
  text people write, such as a list of numbered revisions with comments, can be a generated path
  too. Two changes that each append the next row then combine instead of being held. The queue
  settles the file's conflicts on the target's side, as for any generated file, so such a command
  must merge the file itself from the versions at `MQ_CHANGE_BASE`, `MQ_PARENT` and `MQ_HEAD`
  (`git show "$MQ_HEAD:path"`), rewrite the parts it owns, and treat a conflict anywhere else as
  real. To report one, it prints `merge-queue-conflict: TEXT` naming the file and the lines, and
  exits nonzero. The change is then ejected as `conflict` ("rebase and resubmit") instead of as
  a generator failure. A command that ignored those conflicts would silently drop the change's
  side of them.
- **Refusals.** A change that cannot be regenerated is ejected with a kind that says why:

  | Kind | Meaning |
  |---|---|
  | `regen-build-failed` | `--regen-build` failed: the combined tree cannot even build the generator (stale against the base, or the combination broke the generator's sources). The reason carries the build error's last lines. |
  | `regen-failed` | The regeneration command itself failed (a wrong argument, a generator crash). |
  | `conflict` | The command failed and printed `merge-queue-conflict:` lines: a real conflict in a partly hand-written file. The reason carries those lines. |
  | `regen-touched-other` | The command changed a file that is not generated. |
  | `regen-left-conflict` | Conflict markers remain in a generated file afterwards. |

  Before blaming a change, the queue regenerates the tree just before it. If that fails too, the
  base or the generator's environment is broken, and ejecting each change in turn would eject them
  all; instead the run stops with the reason and nothing is ejected. A failure to start a
  process at all is likewise a run-level error, not an ejection. The kind is recorded on the
  ejection record, in the run report, in the per-change ejection file and in the notification the
  daemon sends.

This is deterministic, not repair. The command must give the same output for the same sources.
The run report lists every regeneration under `regenerated`, with the command's notes.

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
