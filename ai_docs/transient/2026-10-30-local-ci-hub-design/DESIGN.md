# local-ci-hub: phase-1 evaluation and design

Status: phase 1, for coordinator review. No code moved yet.
Scope: promote the originating project's `ci-hub` into a general
agent-utils tool named `local-ci-hub`, simplify its CLI, keep the
originating project working at every migration step.

Method: read the full `ci-hub` tree, ran `ci-hub help` and
`ci-hub quickstart` as a first-time user, inspected the health
tick configuration and its callers, and compared layout against
the existing suite tools `dagrun`, `tick-hub`, and `wrkslots`.
Sizes below are source lines (Python/Rust/shell) excluding tests
and caches unless stated; file counts include tests.

## 1. Holistic evaluation

### 1.1 What ci-hub actually is

`ci-hub` is not one tool. It is at least six tools sharing a name
and a directory:

1. A validation admission and execution engine (detached runs,
   a slot lock, run handles, retained logs and artifacts).
2. An append-only validation ledger (Git-backed JSONL events,
   spool, publisher, compaction, retention, replay caches).
3. A repeated-measurement series store (per-cell history in a
   second Git-backed ledger, with its own publisher).
4. A GitHub reader (open-PR status, main health, runner health,
   manual workflow dispatch, commit-status publication).
5. A landing system (serialized land lock, rebase, exact-head
   receipt checks, merge, ancestry proof).
6. An operations health system (dozens of probes driven by the
   suite `tick-hub` engine, plus escalation and notification).

Around those sit a compatibility website, task/ownership
reconciliation, review attestation, and a large `bin/` of
single-purpose scripts. The front door is one Rust program
(`ci-hub.rs`, about 24.6k lines) that `#[path]`-includes about
twenty library modules; a 49-line shell wrapper runs it through
`rust-script`. Most subcommands are typed in Rust; several are
passthroughs to Python tools whose real parser owns `--help`.

Total scale: about 590 Python files and 1,750 files overall in
the tree. Indicative source sizes:

| Area | Files | Source lines | Role |
| --- | ---: | ---: | --- |
| `validate/` | 370 | ~114k | admission, run lifecycle, receipts |
| `health/` | 431 | ~97k | health probes and tick config |
| `lib/` | 114 | ~89k | shared Rust/Python libraries |
| `series/` | 76 | ~67k | per-cell measurement store |
| `tests/` (top level) | 179 | ~58k | cross-cutting tests |
| `ledger/` | 126 | ~42k | validation ledger |
| `compatibility-website/` | 31 | ~27k | static scorecard site |
| `ci-hub.rs` | 1 | ~25k | the typed front door |
| `landing/` | 92 | ~15k | serialized PR landing |
| `bin/` | 66 | varies | single-purpose scripts |
| `closure/`, `directives/`, `history/`, `runners/`, others | — | 1–7k each | see inventory |

The core ideas are sound and worth generalizing: exact-commit
receipts, an append-only ledger that local and remote evidence
both feed, admission control so parallel agents do not oversubscribe
a dev box, and health as scheduled probes rather than folklore.
The problem is surface, not concept: one name covers all of it,
the defaults name one project's repositories, and the help mixes
daily commands with internal projection plumbing.

### 1.2 Inventory: entry points

Top-level CLI (`ci-hub <COMMAND>`, 41 commands in five help
groups). Verdicts: **G** = generalize into `local-ci-hub`,
**W** = stays a project wrapper, **D** = delete or fold away,
**I** = internal plumbing, remove from top-level help.

| Command | What it does | Used by | Verdict |
| --- | --- | --- | --- |
| `quickstart` | prints the 5-step agent workflow (pure) | agents, humans | G (rewrite) |
| `health` | combines main, PR, ownership health | tick, agents | G (core) |
| `active-work` | reconciles tasks, owners, live agents | coordinator only | W |
| `main-health` | current-main GitHub workflow health | `health`, tick | G (under `health`) |
| `pr-status` (`fresh`) | fresh open-PR GitHub status | agents, tick | G (under `remote`) |
| `tick` | runs due health gates via tick-hub | systemd timer | W (config, not CLI) |
| `verify-landing` | verifies a replay/merge SHA | landing, audits | G (under `land`) |
| `refresh-history` | ingests GitHub + local history | scheduled, manual | G (under `ledger`) |
| `history` | queries retained GitHub/local timeline | diagnosis | G (under `ledger`) |
| `green-time` | carried-forward green/red estimate | diagnosis | W or later |
| `local-history` | recent local validation runs | agents | G (under `run`) |
| `runner-health` | self-hosted runner summary | capacity checks | G (under `remote`) |
| `load-probe` | CPU/memory pressure gate | timing-sensitive runs | G (small, standalone-worthy) |
| `validate-status` | runs by SHA/PR/run number, timelines | everyone | G (core, under `run`/`receipt`) |
| `signal-crosscheck` | reconciles all signals for one SHA | diagnosis | G (under `receipt`) |
| `hosted-status` | exact-head GitHub Actions result | landing | G (under `receipt`) |
| `receipt-digest` | canonical digest of one ledger row | scripts, proofs | I (library/test surface) |
| `admission-disjoint-proof` | proves a disjoint-main floor from Git | admission readers | W (project policy) |
| `validate-evidence` | verifies retained artifacts for rows | audits | G (under `receipt`) |
| `validation-row` | selects one terminal row from JSONL | website, scripts | I |
| `compatibility-website*` (4 commands) | site build/serve and projections | nightly site | W (project site) |
| `validate-run` | launches detached validated run | everyone | G (core, `run`) |
| `validate-stop` | stops detached validate units | operators | G (under `run`) |
| `correct-owner-watch-handles` | repairs pre-service refusals | rare recovery | W |
| `ledger` (3 subcommands) | qualified rows, red attribution, backend compare | agents, diagnosis | G (core) |
| `newest-green` | newest ledger-qualified green commit | recovery | G (under `ledger`) |
| `first-bad` | newest PASS-to-FAIL transition | bisection | G (under `ledger`) |
| `apply-local-label` | reconciles PR label with receipt truth | landing loop | W (label name is project policy) |
| `publish-commit-status` | publishes receipt as commit status | landing loop | G (under `receipt`) |
| `review-attest` | verifies review comment, reconciles labels | review flow | W |
| `land-lock` | fleet landing mutex | landing | G (under `land`) |
| `validate-lock` | validation slot admission (GRANT/QUEUE/REFUSE) | validate-run | I/G (engine, not daily CLI) |
| `ci-mode` | GH Actions admission mode switch | coordinator | W |
| `batch` | named GH Actions batch editing | coordinator | W |
| `ci-timeout` | cancels starved hosted runs, reroutes local | coordinator | W |
| `compatibility-website` | build/serve the site | nightly | W |

`ci-hub/bin/` scripts (66 entries). The generalizable few:
`manual-run` (dispatch a workflow by hand; general pattern),
`load-probe` and `main-health`/`pr-status` (thin aliases of CLI
commands; fold into the CLI), `reconcile-receipts` (7-line alias;
fold into `ledger`), `who-am-i` (agent disclosure tag; already a
suite concern, not CI), `tool-cost` (cost-reporting convention;
suite-wide, not CI). The rest are project operations and stay
with the project: verified git/gh wrappers (`gh`,
`gh-merge-verified`, `git-push-verified`, `gh-label`), task and
chat tooling (`close-task`, `claim-edit`, `notify-coordinator`,
`chat-*`, `tg-*`), escalation machinery (`escalate`,
`*health-escalation*`), and mirror/pin maintenance
(`upstream-mirror`, `ensure-mirror-reaches`). Several `*-test`
files sit beside the scripts they test; they move with them.

Subsystems not on the CLI: `ledger/` (storage, publisher,
retention, compaction — the generalizable core), `series/`
(per-cell store; generalizable later, project-shaped today),
`validate/` (the run engine; its largest file, `start_unit.py`
at ~18k lines, mixes admission, systemd lifecycle, publication,
and cleanup), `health/` (78 probe modules; the probes are
project policy, the tick engine is already the suite's
`tick-hub`), `landing/` (`land-pr.sh` ~56k bytes plus rebase
machinery; the sequence is general, the policy is not),
`compatibility-website/` (project scorecard; stays),
`closure/`, `directives/`, `pin/`, `parity/`, `ratchet/`,
`remediation/`, `stranded/`, `taskgraph/`, `timeout_audit/`,
`greentime/`, `main-metrics/`, `history/`, `runners/` (mostly
project operations; `runners/` runner lifecycle scripts are
host-shape-specific).

Still used? The tick configuration (`health/tick-hub.yaml`,
~2,660 lines) names about 85 scheduled reminders, and the systemd
timer fires the tick every 30 minutes, so the health system is
live, not vestigial. The ledger and validate paths are on the
critical landing path (the project rule is: exact-head ledger
receipt authorizes landing). The website and series store run
nightly/hourly. Genuinely questionable usage is listed under
deletions in section 5.

### 1.3 First-time user report

Ran exactly what the README offers: `ci-hub help`, then
`ci-hub quickstart`. Both are pure, fast, and well grouped; the
five help groups and the 5-step quickstart are genuinely good.
Then the confusions, in the order a newcomer hits them:

1. The quickstart's step 1 says to follow a project skill for
   validation authority. A new project has no such skill, so the
   first instruction points outside the tool. The tool never
   states its own default authority rule.
2. `ci-hub validate-run --help` does not come from the CLI at
   all: it forwards to a Python launcher and prints dozens of
   flags, most of them internal lifecycle verbs (`--retire-unit`,
   `--publish-unit`, `--publication-stopped`,
   `--request-cleanup-worker-json`). The daily flags (`--checkout`,
   `--agent`, `--target`, `--pr`) are buried in that list. The
   Rust source documents this as a measured usability trap that
   was fixed by disabling the front door's own help flag; the
   fix helps, but the surface is still an internal API.
3. There are 41 top-level commands. Four exist only to feed the
   compatibility website's projections, and several others
   (`validation-row`, `receipt-digest`,
   `compatibility-website-text-scan`) are stdin/stdout plumbing
   for scripts. A newcomer cannot tell daily commands from
   internal ones; the help groups do not mark the difference.
4. Defaults name the originating project's repositories
   (`--repo` defaults, canonical-repo constants in the Rust
   source). Nothing says "configure your project here"; the
   configuration is compiled in.
5. `ci-hub ledger --help` shows only three subcommands
   (qualified rows, red attribution, backend comparison). The
   ledger operations a newcomer expects (append, publish, query
   by commit, newest green) live under other top-level names
   (`refresh-history`, `newest-green`, `first-bad`,
   `validate-status`), so "the ledger" as an object is not
   discoverable from its own subcommand.
6. Two commands answer "did this commit pass?": `validate-status
   --sha` (local ledger) and `hosted-status` (GitHub). Which one
   authorizes what is policy the help does not state; locally
   the answer is "the ledger", but that lives in the skill, not
   the tool.
7. Everything assumes it runs from the project root, that
   `with-proxy` exists, that systemd user services are available,
   and that a Herdr workspace should appear. None of these
   prerequisites is checked or named up front; failures surface
   deep inside a run.
8. Every finite command appends wall+CPU cost lines. Useful for
   operators, noise for a first run, and undocumented until the
   tool-cost convention document is found.
9. The README is 614 lines and front-loads edge cases (historical
   website snapshots, frozen validation, NO_RESULT carve-outs)
   before the ordinary loop. The quickstart is the real entry
   point; the README reads as a contract archive.

Net: an agent can drive it (the quickstart plus `--help` purity
is agent-friendly), but a human or agent from another project
cannot tell what to configure, what is safe to ignore, or which
of the 41 verbs matter. That is the simplification target.

## 2. Proposed `local-ci-hub` CLI

Six orthogonal subcommands. Each owns one object; no verb
appears in two places; internal plumbing is a library, not a
command. Every subcommand accepts `--json`; read-only commands
never write; `--help` never writes, touches the network, or
imports heavy optional dependencies (keeping the current purity
contract, which is good and should become a tested invariant).

| Command | One-line purpose |
| --- | --- |
| `local-ci-hub run` | Admit, launch, attach to, stop, and list validation runs on this machine. |
| `local-ci-hub receipt` | Answer "what is proven about this exact commit?" from local and remote evidence. |
| `local-ci-hub ledger` | Append, publish, and query the append-only evidence ledger. |
| `local-ci-hub remote` | Read and dispatch remote CI (GitHub Actions) without granting it authority by itself. |
| `local-ci-hub health` | Report machine, queue, and project health from configured probes. |
| `local-ci-hub land` | Serialize and execute a landing that consumes a qualifying receipt. |

Shape within each (flags finalized in phase 2):

- `run start|attach|stop|list|status` — absorbs `validate-run`,
  `validate-stop`, `validate-status` (run views), `local-history`.
  Admission (today's `validate-lock`) is the engine under
  `run start`, configurable slot count, not a daily verb.
- `receipt show|verify|publish` — absorbs `validate-status --sha/--pr`
  verdicts, `hosted-status`, `signal-crosscheck`,
  `validate-evidence`, `publish-commit-status`. One verdict
  object names the commit, the evidence sources consulted
  (local ledger, remote checks), and the qualification rule that
  decided it. `receipt-digest` becomes a library function with
  tests, not a command.
- `ledger append|publish|ingest|query|newest-green|first-bad|attribute|compare` —
  absorbs today's `ledger` trio (`qualified-rows` to `query`,
  `attribute-reds` to `attribute`, `backend-factor` to
  `compare`) plus `refresh-history` (as `ledger ingest`),
  `history`, `newest-green`, `first-bad`, and
  the `reconcile-receipts` alias. The ledger is one discoverable
  object again. Ledger query commands report recorded outcomes;
  only `receipt` applies the current qualification policy, so a
  policy change never silently rewrites historical answers.
- `remote status|dispatch| runners` — absorbs `pr-status`,
  `main-health`, `runner-health`, `manual-run`. Dispatch is
  explicit (workflow, ref) and reports; it never gates.
- `health [--gate]` — runs the project's configured probes
  (section 3) once or through the suite `tick-hub` engine on a
  timer the project installs. Absorbs `health`, `load-probe`
  (as a built-in probe), and the tick entry point.
- `land acquire|release|run|verify` — the landing mutex and the
  post-merge ancestry proof (`land-lock`, `verify-landing`).
  The merge sequence itself (rebase strategy, merge command)
  stays a project script that calls these.

Plus three meta commands: `quickstart` (the configured project's
own 5-step loop, generated from its config, not compiled in),
`userguide` (the full operator guide, per suite convention), and
`doctor` (prerequisite checks: git, ledger reachability, remote
credentials, service manager, proxy — the checks whose absence
caused confusion 7 above).

What this deletes from the surface: the four
`compatibility-website*` projection commands, `validation-row`,
`receipt-digest`, `admission-disjoint-proof`,
`correct-owner-watch-handles`, `batch`, `ci-mode`, `ci-timeout`,
`active-work`, `green-time` (returns later only if a second
project wants it). They remain project wrappers or library code;
they stop being top-level verbs a newcomer must classify.

Language and layout follow the suite's paired-tool convention
(see section 6): a Python package under `py/local_ci_hub/` with
`py/bin/local-ci-hub`, a Rust edition under `rs/local-ci-hub/`
when a component earns it (the ledger fold and receipt digest
are the candidates; they are already Rust in the source system),
the tracked resolver link under `common/bin/`, docs under
`common/docs/local-ci-hub/`, and a skill under
`skills/local-ci-hub/`. Phase 2 may start Python-only with the
mirror state recorded as `absent`, which the editions model
already supports; that decision is flagged for the coordinator
because the source system's ledger types are Rust-owned today.

## 3. Configuration model

One file per project, committed in the project repo, discovered
from the checkout root (or `--config`). Working name:
`local-ci-hub.toml`. Everything compiled into today's binary
becomes data here:

```toml
[project]
name = "example"
# Repositories this project validates, by role. No defaults.
repos = [{ id = "product", remote = "owner/product", default_branch = "main" }]

[validate]
# The project's own validation command. local-ci-hub runs it;
# it does not know what the command does.
command = ["make", "validate"]
# Named test cells the command can select, for first-bad and
# per-cell queries. Empty means "one unnamed cell".
cells = []
admission_slots = 6        # parallel runs on this machine
run_deadline = "2h"

[ledger]
# Append-only JSONL ledger. Exactly one of: a directory inside
# this repo, or a separate ledger repo checkout.
location = { repo_path = "ci-ledger" }   # or { path = "ledger" }
team = "example-team"                    # shard key, with short host name
publish = "git"                          # git | none (local-only)

[remote]
provider = "github"        # github | none
dispatch = "manual"        # manual | automatic (reported, never assumed)
workflows = ["ci.yml"]

[receipt]
# The qualification rule, stated in the tool, not a skill:
# which evidence qualifies a commit for landing.
qualifying = ["local-ledger-pass"]       # e.g. + "remote-pass"
publish_commit_status = false

[health]
probes = []                # project probe commands, tick-hub schema
tick_config = "ci/health.yaml"

[land]
lock_path = "ignored/local-ci-hub/land.lock"
merge = ["gh", "pr", "merge", "--rebase"]   # project-owned sequence hook
```

Rules: no repository, host, or team name is ever defaulted;
`doctor` refuses a config with missing required fields and names
them; short host names (never fully qualified) key the ledger
shards, as today; secrets and credentials are never read from
this file. A project with `provider = "none"` and
`publish = "none"` gets a fully local tool, which must be a
supported, tested configuration.

## 4. What stays as project wrappers

The originating project keeps, in its own repo, as thin wrappers
over `local-ci-hub` (each a config value or a small script, not a
fork):

- Its `local-ci-hub.toml` (repos, validate command, cells,
  ledger location, qualification rule, probe list).
- The landing sequence script (rebase strategy, union-rebase,
  label reconciliation, review attestation): it calls
  `land run`, `receipt show`, and its own merge steps.
- The health probe set (the 78 probe modules and the tick
  configuration): these are project policy by design; the tool
  supplies the probe protocol and the tick engine already in
  the suite.
- The series/per-cell store and its scorecard website, until a
  second project needs per-cell history; then they generalize
  behind `ledger` as an optional cell-evidence extension.
- Verified git/gh wrappers, task/chat/escalation scripts, and
  mirror/pin maintenance in `bin/`: unrelated to CI evidence.
- The validation-authority skill: it shrinks to naming the
  project's config and its qualification rule, instead of
  restating tool behavior.

## 5. What gets deleted as dead

To be confirmed per item in phase 2 by caller search before
deletion (the tick config and landing path are live; nothing on
those paths is proposed for deletion here):

- Top-level surface only (code survives as library/wrapper):
  the four `compatibility-website*` projection commands,
  `validation-row`, `receipt-digest` as commands.
- Duplicate aliases: `bin/load-probe`, `bin/main-health`,
  `bin/pr-status`, `bin/reconcile-receipts` thin wrappers once
  the CLI covers them.
- The `*-test` scripts sitting in `bin/` move beside their
  tools' real test suites or are deleted if the suite already
  covers them (`ensure-mirror-reaches-test`,
  `upstream-mirror-test`, the `gh-merge-verified` test pair are
  tests, not entry points).
- `compatibility-website` historical-snapshot machinery stays
  with the project but leaves the general tool entirely.
- Any health probe whose tick reminder was already removed and
  which has no remaining caller: candidates are identified by
  grepping the tick config for each probe module in phase 2;
  the 2026-08-08 removal record in the tick config shows this
  has happened before and left modules behind.

Explicitly not deleted: ledger history (append-only means
history is never rewritten; compaction and retention already
exist and carry over), retained run evidence, and the series
store's published rows.

## 6. Suite conventions to follow (from dagrun/tick-hub/wrkslots)

- Paired layout: `py/<tool>/` package plus `py/bin/<tool>`
  launcher, `rs/<tool>/` plus `rs/bin/<tool>` when a Rust
  edition exists, one tracked resolver link in `common/bin/`,
  and an entry in `validation/editions.json` stating primary
  and mirror state honestly (`absent` is a valid, declared
  state, not an omission).
- Docs: `common/docs/<tool>/` with the user guide rendered the
  same way as `tick-hub`'s, CLI `quickstart` and `userguide`
  output as the primary operator documentation, every argument
  documented with defaults and units.
- A skill under `skills/<tool>/SKILL.md` (not edited in this
  phase; the coordinator/owner rule applies — phase 2 proposes
  its content for approval).
- Registration in `validation/components.json` with real
  `source_prefixes` and `test_files`, so `make validate`
  selects this tool's checks for its paths and an unclassified
  path still selects everything.
- Tests beside the package (`py/tests/test_local_ci_hub_*.py`
  convention), including the `--help` purity invariant and a
  local-only configuration end-to-end test.
- No client-project names anywhere in the tool, its tests, or
  its docs; examples use a neutral placeholder project.

## 7. Migration order (every step leaves the project working)

1. **Config first.** Write the project's `local-ci-hub.toml`
   describing today's behavior exactly (repos, validate
   command, ledger location, qualification). No code changes;
   the file is initially read by nothing. Reviewable on its own.
2. **Ledger library.** Land `local-ci-hub ledger` read paths
   (query, newest-green, first-bad) in agent-utils against the
   project's existing ledger location, read-only. The project's
   writers are untouched; prove byte-identical query results
   against today's commands on retained history.
3. **Receipt verdict.** Land `receipt show` combining local
   ledger and remote status behind the configured qualification
   rule. The project's landing script gains an optional flag to
   consult it; default remains today's path until results match
   on live PRs.
4. **Run engine.** Land `run start/attach/stop/list` over the
   project's existing run-handle and spool directories, so old
   and new front doors observe the same runs. Switch the
   project's validate entry point to the new command; the old
   passthrough remains as a wrapper for one release.
5. **Remote and health.** Land `remote` and `health`; point the
   tick configuration's probes at the new health entry point
   without changing probe code or cadences.
6. **Land.** Move the lock and verify steps behind `land`; the
   project landing script shrinks to its policy steps.
7. **Surface removal.** Only now delete the folded top-level
   commands and `bin/` aliases from the project, and retire the
   old front door. Each prior step is independently revertible
   by flipping the wrapper back, because state formats never
   changed: ledger events, run handles, and spool layout are
   adopted as-is and versioned, not migrated.

State-format rule for all steps: `local-ci-hub` reads the
existing `validate-ledger/v1` envelope and run-handle schema as
its v1 formats. Any future format change is a new schema version
with a reader that accepts both, never a rewrite of published
history.

## 8. Phase 2–4 preview

- Phase 2 (extract): `py/local_ci_hub/` with `run`, `ledger`,
  `receipt` first (the brief's core), then `remote`, `health`,
  `land`; tests per subcommand; `make validate` green on the
  new component. Open decision for the coordinator: Python-only
  start (mirror `absent`) versus porting the Rust ledger types
  immediately. Recommendation: Python-only, because the fold and
  digest are small, already specified by tests in the source
  system, and can be ported behind the same CLI later; the risk
  is digest/canonicalization drift, mitigated by golden tests
  over retained ledger rows in step 2 above.
- Phase 3 (port): the coordinator applies the wrapper patch on
  the project side; this lane prepares the exact patch plus the
  submodule-pin test plan (point the project's agent-utils
  submodule at this branch in a scratch harness, run the
  project's own validation, compare receipt verdicts).
- Phase 4 (template): draft the minimal template as a recipe
  first ("set me up a multi-agent workspace using agent-utils"
  plus a `local-ci-hub.toml`), and only propose a template repo
  for what the recipe cannot express. No repository is created
  without the owner's explicit go-ahead.

## 9. Risks and open questions

1. Canonicalization drift between the Rust-owned ledger types
   and a Python reimplementation is the top correctness risk;
   golden tests over real retained rows are mandatory, and the
   digest function should be the first thing ported with tests.
2. The run engine's systemd/Herdr observer coupling is
   host-shape-specific; the general tool should treat the
   observer as an optional hook, with detached execution via
   the suite's existing mechanisms as the default.
3. `start_unit.py` (~18k lines) mixes at least four
   responsibilities; phase 2 should extract by behavior with
   characterization tests, not transliterate it.
4. Whether `land` belongs in the tool at all, or only the lock
   plus verify primitives: the proposal includes it because
   two projects landing through receipts is the owner's stated
   direction, but a lock-only `land` is an acceptable smaller
   scope if the coordinator prefers.

## 10. Independent review (Codex, read-only) and responses

A read-only Codex review of this design was run in phase 1; its
full text is preserved beside this file in the lane's scratch
output and summarized here with dispositions. Verdict: revise
before switching writers or landing authority. Accepted:

1. **`land` must have one transaction owner.** Revised: the
   project script owns orchestration (rebase, merge, labels);
   `land` provides only `acquire|release|verify` primitives and
   a `run` that executes one project-supplied command while
   holding the lock. It never merges itself. Section 2's `land`
   row should be read with this narrowing.
2. **Qualification semantics must be specified, not sketched.**
   Phase 2 must define, before any writer switch: AND/OR
   composition, required cells/contexts, treatment of pending,
   skipped, cancelled, missing, expired, and conflicting
   evidence, retry supersession, trusted producers, exclusion of
   self-published commit status from its own evidence, and the
   receipt's binding to repo, exact revision, validation scope,
   evidence IDs, and policy/config version. A change under test
   must not be able to weaken its own landing policy: the
   landing path reads policy from the target branch's committed
   config, not the candidate checkout's.
3. **Ledger write-side migration was missing.** Migration gains
   an explicit step between today's steps 2 and 4: name every
   writer (appender, publisher, reconciliation, compaction,
   retention) and its lock, transfer one at a time, and prove
   old-reader/new-writer and new-reader/old-writer compatibility
   in both directions before the execution switch.
4. **Python reimplementation of canonicalization is not
   justified by query equivalence.** Revised recommendation:
   keep the Rust-owned fold/digest as the implementation behind
   the new front door initially (a Rust library/bin the Python
   CLI calls), and only port with differential tests over exact
   canonical bytes, digests, null-vs-missing, Unicode, numeric
   edges, unknown fields, duplicate/replayed/truncated events.
   This changes section 8's recommendation; the coordinator
   decides the edition strategy in phase 2.
5. **Lock scope.** `land` and admission locks are machine-scoped
   (all checkouts on one host), with paths under the configured
   runtime state root, never checkout-relative; cross-host
   serialization, if wanted, is a separate explicit mechanism.
6. **Compatibility removals need replacement invocations.**
   `receipt-digest` and `validation-row` keep a shell-callable
   form (under `receipt digest` / `ledger row` or as documented
   library entry points with a stable CLI shim) until every
   caller is migrated; deletion requires a caller inventory
   covering imports, shell wrappers, timers, and docs, not a
   tick-config grep alone. Tests are never deleted on a coverage
   claim without an assertion-level mapping.
7. **First-time path.** Phase 2 deliverables add: a minimal
   local-only config example, a capability matrix (which
   commands need GitHub, systemd, or a proxy), the
   start-observe-inspect loop in `quickstart`, versioned JSON
   schemas with stable exit codes, and a `RELATED_WORK.md` for
   the tool (a suite requirement this draft initially omitted).

Not accepted as stated: none. Two items are deferred rather
than rejected: exact enumeration of all 41 commands with
aliases (the table groups the four website projection commands;
phase 2 starts from `ci-hub help` output as the checklist), and
the minimum cell evidence the core ledger retains without the
series store (a phase-2 schema decision, flagged in section 9).
