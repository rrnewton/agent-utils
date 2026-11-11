# Task-tracker adapters: a two-tier task policy, one task interface, any tracker CLI

#631 task-tracker-adapters, 2026-10-28. A design for the owner's review, not user documentation.
Near-term fixes are already filed as issues (section 8); the bigger features wait for this
review.

## 1. The request

The owner, 2026-10-28, in short:

- **Two tiers.** GitHub issues are for serious bugs and for reports from other teams and people.
  Task dispatch goes through the local tracker (minibeads here). The split keeps churn off GitHub,
  away from its rate limits and away from the real bug reports.
- **The local tier is a dispatch queue, not a dump.** A task is dispatched to a subagent, and that
  subagent closes it when the implementation lands. Landing and draining is the worker's job
  first and the coordinator's as a backstop.
- **A la carte.** No tool may assume minibeads. There should be a generic task interface, and a
  new tracker CLI should be usable from a JSON configuration that maps actions onto its commands.
  minibeads, upstream beads, GitHub, Gas Town and internal trackers are the examples.
- **minibeads joins the suite** as a git submodule, on the website and in the tool list, with the
  message that agent-utils works with it and with other trackers.
- Questions to answer: is the workflow functioning, is the coordinator checking the backlog, are
  tasks linked in dependency order?

## 2. What actually happens today

Measured on 2026-10-28 from this repository's `.minibeads/` at main de9ad62e (`mb --json list`,
499 issues before this change) and from one `gh issue list --state all` call (83 issues).

| Question | Answer | Evidence |
|---|---|---|
| Are tasks filed and closed? | Mostly, after the fact. | 451 of 499 closed. Median create-to-close is 2.0 h (90th percentile 25.5 h), which says most issues are filed around the time the work lands, not when it is dispatched (an inference from the timing). |
| Is dispatch recorded? | No. | 0 issues have ever been `in_progress` at a snapshot taken today; 35 of 499 have an assignee, with free-form values (model names, a host name). |
| Is the coordinator checking the backlog? | Not systematically. | It kept a private notes file of dispatchable work; 11 such entries were found and filed today as #637-#647 (listed in #633 backlog-to-tracker). 13 open issues had not been touched in 7 days. |
| Are tasks linked in dependency order? | No. | 80 issues have any dependency, but only 5 `blocks` edges exist, all added today on #624 voice-page-modularity. The rest are `parent-child` (52), `related` (27) and `discovered-from` (8), none of which gate `mb ready`. |
| Would dependencies have worked? | **No.** | The `mb` the harness runs (0.25.0) keeps treating a *closed* blocker as blocking: in a scratch tracker, B blocked by A, then A closed, `mb ready` lists nothing and `mb blocked` still lists B. Upstream fixed it (a210c08, "Resolve prerequisite status when computing ready and blocked work"), first released in 0.29.0; the same scratch test on a 0.29.0 build lists B as ready. #624 voice-page-modularity's description promises "mb ready lists it once all of them are closed"; with 0.25.0 it never would. |
| Is GitHub triaged? | No. | 19 open issues (5 from August); one issue closed since 2026-10-14; 6 filed in the last two days by agents of other projects, unanswered. |
| Does anything watch the trackers? | No. | tick-hub runs generic health checks only, and no tick-hub instance runs for this harness. agentctl and wrkslots have no notion of a task id (wrkslots `--task` is a free-form slug). |

So the two-tier idea exists only as habit. The local tier records outcomes rather than driving
dispatch, nothing marks work in flight, and the tool that would have made dependency order useful
silently ignored closed blockers.

## 3. The two-tier policy

| | Report tier (GitHub) | Dispatch tier (local tracker) |
|---|---|---|
| Holds | Serious bugs; reports from outside (other teams, other projects' agents, humans); anything a person outside the harness must be able to find. | Every unit of work a coordinator dispatches or might dispatch: features, fixes, follow-ups, flakes, chores. |
| Written by | Reporters; the coordinator when triaging. | The coordinator at dispatch; workers when they find follow-ups; the worker closes its own. |
| Volume | Low. Paced through gh-paced; shared rate limits. | High. Local files, no network, no rate limit. |
| Lifecycle | Triage within 3 days: close (fixed, with the commit; or not planned, with the reason), or label `triaged` and mirror into the dispatch tier. | open, then in_progress at dispatch (claimed in the agent's name), then closed by the worker when the work is on the default branch; the coordinator checks at stop. |

Rules that make it work:

1. **One mirror, one direction of truth.** A GitHub issue that needs work gets one dispatch-tier
   issue whose external reference is `gh-<n>`; the GitHub issue gets one comment naming it. Work
   happens in the dispatch tier; when it lands, the worker closes both (minibeads' own
   `mb github link/sync` can do the GitHub half; another tracker uses the generic external
   reference).
2. **Claim at dispatch, close at land.** The coordinator claims the issue in the agent's name when
   it starts the agent; the brief names it; the worker closes it with the landed commit; the
   coordinator, at stop, finds it closed or finds a comment saying what remains.
3. **Real dependencies use `blocks`.** `related` and `discovered-from` are notes; only `blocks`
   orders work. An epic's children block nothing by default; give them `blocks` edges when order
   matters.
4. **Say which tier a number belongs to.** GitHub and minibeads numbers overlap (both run into the
   hundreds), so `#290` is ambiguous. Write `GitHub #290` for the report tier and keep `#<n> <slug>`
   for the local tier, as AGENTS.md already asks.
5. **The private notes file holds only what cannot be an issue**: host-private operations and
   decisions waiting on the owner.

## 4. The generic task model

Every tracker surveyed (section 6) has these concepts; the adapter maps each to the tracker's own
spelling, or declares it missing.

| Concept | Canonical form | Notes |
|---|---|---|
| id | an opaque string; `display` renders it (`#{id}` for GitHub) | Unique within one tracker instance. Never parse it outside the adapter. |
| title, description | strings | |
| state | `open`, `in_progress`, `blocked`, `closed` | `blocked` is either stored, or derived: an open task with at least one open `blocks` dependency. A tracker's custom statuses map through categories (section 5.4). |
| priority | integer 0 (highest) to 4 | Matches minibeads and beads; a tracker without priority emulates with labels or reports none. |
| type | bug, feature, task, epic, chore, or the tracker's own | Advisory. |
| labels | set of strings | Some trackers only replace the whole set; the adapter says so. |
| dependencies | `blocks` edges (task -> blocker), plus `parent` | Only `blocks` affects readiness. |
| claim | assignee plus optional lease | Atomic where the tracker can (lease, compare-and-swap, claim-next); otherwise assign then verify. |
| comments | append-only text | Close reasons go here where the tracker has no reason field. |
| links | `external_ref` (`gh-290`), `url` | How the two tiers point at each other. |
| timestamps | created, updated, closed | Drive staleness checks. |

Operations: `list` (filter by state, label, limit), `show`, `ready`, `blocked`, `create`,
`set_state`, `claim`, `release`, `close` (with reason), `reopen`, `comment`, `label_add`,
`label_remove`, `dep_add`, `dep_remove`. Optional orchestrator operations (section 5.6):
`dispatch`, `group`, `complete`.

## 5. The adapter format: `agent-utils/task-adapter@0`

One JSON file per tracker teaches every tool in the suite to drive that tracker's own CLI. Four
examples sit beside this note, each stamped with how far it was verified:

| File | Tracker | Verified |
|---|---|---|
| [`adapters/minibeads.json`](adapters/minibeads.json) | minibeads (`mb`) | Read actions run against this repository through a prototype interpreter; writes built and shown, not run. |
| [`adapters/beads.json`](adapters/beads.json) | upstream beads (`bd`) | Written from bd's CLI reference and Go types at upstream 9353122; bd is not installed here. |
| [`adapters/github.json`](adapters/github.json) | GitHub Issues (`gh issue`) | Flag names from `gh` 2.102.0 help; the list JSON shape from one read-only call. |
| [`adapters/gastown.json`](adapters/gastown.json) | Gas Town (`gt`) | Extends the beads adapter; from Gas Town's docs at upstream de2ce8e; not run. |

Adapters for internal trackers follow the same format and live in private configuration, not in
this repository.

### 5.1 Top-level keys

| Key | Meaning |
|---|---|
| `schema` | `agent-utils/task-adapter@0`. A reader refuses other major versions. |
| `name`, `summary`, `verified` | Identity, one line for humans, and what was actually exercised. |
| `program` | argv prefix (`["mb"]`, `["gh", "issue"]`). Never a shell string. |
| `json_args` | Appended for actions whose output format is `json` (`["--json"]`). |
| `actor_args` | How to pass the acting agent's name (`["--actor", "{actor}"]`), if the tracker records one. |
| `params` | Instance settings the project supplies (GitHub's `repo`, a namespace, a database name). |
| `detect` | Files that mark a project as using this tracker, and a probe command. |
| `id` | `pattern` (to reject a malformed id before running anything) and `display`. |
| `states`, `state_categories` | Canonical state -> native values (section 5.4). |
| `priority` | Native scale, highest, lowest, default; `null` when the tracker has none. |
| `fields` | Canonical field -> JSON path into one task object (section 5.3). |
| `capabilities` | What the tracker does natively and how a missing piece is emulated (section 5.5). |
| `actions` | Canonical operation -> command template and output parser (section 5.2). |
| `extends` | Inherit another adapter and override or add keys (Gas Town extends beads). |
| `orchestrator` | Optional dispatch verbs (section 5.6). |

### 5.2 Command templates

An action's `argv` is a list of words appended to `program`. There is no shell: each word becomes
exactly one argument, so a title containing quotes, `$()` or `;` cannot inject anything.

- `{name}` is a required placeholder; an unbound one is an error before anything runs.
- A nested list is a **group**. `["--label", "{label?}"]` is dropped whole when the `?`
  placeholder is unbound, so optional flags need no conditional syntax.
- `{labels*}` repeats its group once per element (`--label a --label b`); on its own, outside a
  group, it becomes one word per element. `{labels,}` joins the list with commas into one word
  (`--labels a,b`). Both list forms are optional by nature: an unbound or empty list drops its
  group (or, alone, contributes no words).
- Values come from the caller (`id`, `title`, `state`, `assignee`, `reason`, `body`, `blocker`,
  `limit`, ...), from `params`, and from extra top-level string values the adapter defines
  (GitHub's `list_fields`).
- `state` is translated to the native value before expansion. Where a command wants a different
  spelling from what it prints, the action says so with `state_values` (GitHub prints `OPEN` but
  `gh issue list --state` wants `open`; the prototype found this).

Each action also declares its result handling: `ok_exit` (default `[0]`), `conflict_exit` (beads
exits 13 when a compare-and-swap guard lost a race) or `conflict_stderr` (a pattern), and `verify`
(run another action afterwards to confirm, for trackers whose claim is not atomic). Two more
per-action keys: `json: false` skips `json_args` for an action whose output is text even though
the tracker has a JSON flag (beads' `create --silent` prints only the id), and
`actor_is_assignee: true` says the tracker claims for the acting identity (`actor_args`), so no
`{assignee}` is passed.

Actions outside the canonical list are allowed (beads' `stale`, one internal tracker's
claim-next): a tool uses the ones it knows and ignores the rest. Keys ending in `_note`, and
`summary`, are prose for humans.

### 5.3 Output parsing

`output.format` is `json`, `jsonl`, `text` or `none`. For JSON, `items` (a list) or `item` (one
task) select task objects, and `fields` map each canonical field with a deliberately small path
language: `$`, `.key`, `[n]`, `[*]`, and the filter `[?(@.key=='value')]` (for "the ids of
dependencies of type blocks"). A path with `[*]` or a filter yields a list; any other path yields
one value or null. A `create` action's `output.id` is the path to the new task's id. For text,
`regex` with named groups (`(?P<id>...)`) extracts what the action
needs, typically the id a `create` printed. A tracker with no machine-readable output at all is
supported only through `text` and is marked as such.

### 5.4 States

`states` maps each canonical state to the native values that mean it; the first is the one
written. A native value not listed is looked up in `state_categories`, for trackers that let a
project define statuses in categories (upstream beads: active, wip, done, frozen; one internal
tracker has the same idea): active -> open, wip -> in_progress, done -> closed, frozen -> blocked.
Its keys: `probe` (the argv, after `program`, that lists the statuses), `items`, `name` and
`category` (paths into that output), and `map` (category -> canonical state). Anything still
unknown is reported as `unknown`, never guessed.

### 5.5 Capabilities and emulation

Each capability is `native`, an emulation recipe, or `none`. The generic layer emulates only where
the recipe says how, and otherwise refuses with the capability's name, so a tool never silently
drops a step.

| Capability | Values seen | Emulation when absent |
|---|---|---|
| `in_progress` | `native`; `label:in-progress` (GitHub) | A label, plus the assignee. |
| `blocked` | `native`; `derived` | Open with an open `blocks` dependency. |
| `ready_query` | `native`; `derived` | `list` open, minus blocked, minus claimed. |
| `dependencies` | `native`; `none` | Refuse; a body line is not a dependency. |
| `claim` | `atomic-lease`, `atomic-next`, `assign-then-verify`, `agent-field` | Assign, then `show` to confirm nobody else holds it. |
| `close_reason` | `native`; `comment` | Comment, then close. |
| `priority` | `native`; `label:P{priority}` | Labels `P0`..`P4`, or none. |
| `labels` | `native`; `replace-only` | Read, modify, write the whole set. |
| `explicit_ids`, `external_ref`, `parent`, `compare_and_swap`, `change_stream` | per tracker | Reported, not emulated. |

A capability key a reader does not know (GitHub's `rate_limited`) is informational and ignored.

### 5.6 Orchestrator verbs

Some trackers come with their own dispatcher. Gas Town keeps tasks in beads and adds `gt sling
<id> <rig>` (assign the task to a worker and start it), a per-agent hook (the worker's current
task), convoys (groups of tasks) and `gt done` (the worker submits its branch and ends). One
internal agent orchestrator likewise selects its task store per session, between a local database
and a shared tracker. The `orchestrator` block names such verbs, so agentctl can either drive its
own start and stop around the tracker's claim and close, or hand dispatch to the tracker's
orchestrator when a project already uses one. Unlike `actions`, an orchestrator verb's `argv` is the
whole command line: it is not appended to `program`, because the dispatcher is usually a
different program from the task store (`gt` over `bd`). `run_as: worker` marks a verb the
worker, not the coordinator, runs; `effect` is prose.

### 5.7 Where adapters come from

In order: `--adapter NAME|PATH`; the project's own configuration (a small instance file naming
the adapter and its `params`, for example `{"adapter": "github", "params": {"repo":
"OWNER/REPO"}}`, for each tier); the user's adapter directory
(`$XDG_CONFIG_HOME/agent-utils/task-adapters/`, where private adapters live); and the adapters
built into the tool. `detect` picks one when nothing is configured and exactly one matches.

### 5.8 Prototype

A throwaway interpreter of about 150 lines (not committed) implemented the template groups, the
path language and state mapping, and ran the minibeads adapter's `show`, `list`, `ready` and
`blocked` against this repository: every canonical field came out right, including `blocked_by`
through the filter path and `parent` from `parent-child` dependencies. Dry runs of `create` for
minibeads, beads and GitHub produced the expected argv. That is the evidence the format is
implementable in a small amount of code; it is not a test of the beads, GitHub or Gas Town
adapters against live trackers.

## 6. Trackers surveyed

| | minibeads | upstream beads | GitHub (`gh`) | Gas Town |
|---|---|---|---|---|
| Storage | Markdown files in the repository | Dolt database (server or embedded) | GitHub | beads per rig |
| JSON output | `--json` | `--json` | `--json FIELDS` | via bd |
| States | open, in_progress, blocked, closed | the same plus deferred, pinned, hooked, custom statuses in categories | open, closed | beads' |
| Dependencies | `blocks`, parent-child, related, discovered-from | the same, plus gates, external references | blocked-by, sub-issues | beads' |
| Ready query | `mb ready` (closed blockers fixed in 0.29.0) | `bd ready` | none | beads' |
| Claim | `--claim`, lease (`--for`) | `--claim`, lease (a lost claim exits 1, "already claimed"), `--if-assignee`/`--if-status` compare-and-swap guards (exit 13 when a guard fails) | assignee | `gt sling` |
| Close | reason | reason, `--claim-next` | comment plus reason kind | `gt done` |
| Change feed | none | none | webhooks (not via `gh`) | events |

Internal trackers are compared in the private note, not here. Ideas worth borrowing into the
generic layer: beads' status categories and compare-and-swap exit code; an atomic "claim the
next ready task" and a close that records what resolved the task, which map directly onto "the done
verdict closes the task with the landed commit"; Gas Town's split of task store from dispatch
verbs; and minibeads' `mb github` sync as the first two-tier mirror.

## 7. Integration points

Each integration calls the adapter through one small command (section 8, B1) and is off unless a
project configures a tracker, so no tool gains a hard dependency.

- **agentctl.** `agentctl start NAME --task ID`: claim ID in NAME's name (in_progress, assignee,
  lease for the expected run), record the id in the agent's record, and prefix the brief with
  `Task: #<n> <slug>` and the close instruction. `agentctl status/list/tree` show it. When
  watch-agents sees a `DONE:` verdict it adds the task's state to the child-done notice ("task
  still open" is the useful case); with `--close-task-on-done`, off by default, it closes the task
  with the verdict line as the reason. `agentctl stop` warns when the task is still in_progress
  and releases the claim unless told to keep it.
- **wrkslots.** `create --task-id ID` records the tracker id next to the slug; `status` shows it;
  `remove` notes an issue still open with no handoff explaining it. The slot task id and the
  tracker id become the same string.
- **tick-hub.** Today: shell gates over the tracker's JSON (#635 tick-hub-task-health). Later: a `tasks` health check
  kind backed by the adapter, reporting open-task growth, stale open tasks, in_progress tasks
  whose assignee is not a running agent, running agents with no claimed task, tasks that became
  ready since the last tick, and report-tier issues untriaged past N days.
- **The coordinator skill.** The two-tier policy and the claim-at-dispatch, close-at-land
  protocol (#632 task-dispatch-protocol), written for "the project's task tracker", with minibeads as the example.

## 8. Plan

### Near-term fixes (filed, children of #631 task-tracker-adapters)

| Issue | What | Order |
|---|---|---|
| #632 task-dispatch-protocol | File, claim (in_progress) and close every dispatched task; brief template carries the id; coordinator skill says it. | first |
| #633 backlog-to-tracker | Move the private backlog's dispatchable entries into the tracker. | done in this change: #637-#647 |
| #634 github-triage-pass | Triage the 19 open GitHub issues; mirror real ones; label outside reports; close fixed ones. | first |
| #635 tick-hub-task-health | A tick-hub config for this harness that watches both tiers with shell gates. | blocked by #632 and #634 |
| #636 stale-task-triage | Close, re-scope or block the 13 untouched open issues. | after #633 |
| #648 mb-upgrade | Run `mb` 0.29.0 (the pinned submodule), so closed blockers stop blocking. | first |

#648 mb-upgrade matters most: until it is deployed, every `blocks` edge hides its dependent from `mb ready`
for good. 0.29.0 also parses issue files more strictly and refused 8 of this repository's (two
non-standard top-level sections, and text before the first section); this change rewrites those
8 without changing their text, and adds the two transient files 0.29.0 writes to
`.minibeads/.gitignore`. After that, 0.29.0 reads every issue here and 0.25.0 still does.

### Bigger features (for this review; filed after it)

| | Feature | Size | Notes |
|---|---|---|---|
| B1 | A small `taskctl` command: loads an adapter, runs canonical operations, prints normalized JSON; ships the four public adapters built in; a conformance kit (a fake tracker plus recorded CLI transcripts) every adapter must pass. | medium | Rust, like the other new tools. The name collides with an unrelated open-source task runner; alternatives welcome. |
| B2 | agentctl `--task` (section 7). | medium | Calls `taskctl` as an external program when configured; nothing otherwise. |
| B3 | wrkslots `--task-id`. | small | |
| B4 | tick-hub `tasks` checks through `taskctl`. | small to medium | Replaces the shell gates of #635 tick-hub-task-health. |
| B5 | Two-tier mirror: `taskctl mirror` keeps a report-tier issue and its dispatch-tier twin in step through `external_ref`, for trackers without minibeads' `mb github`. | medium | Writes to GitHub go through gh-paced. |
| B6 | Private adapters for the internal trackers, conformance-tested privately. | small each | One of them needs a JSON output upstream first. |

## 9. Staying a la carte

- No suite tool links minibeads or reads `.minibeads/`; everything goes through an adapter file
  and the tracker's own CLI.
- Every integration is off until a project names a tracker; with none, agentctl, wrkslots and
  tick-hub behave exactly as today.
- A new tracker needs one JSON file, not code. The format's capability flags make what a tracker
  lacks explicit instead of silently skipped.
- minibeads is in the suite because it is the reference dispatch tracker and the one this
  repository uses, not because anything requires it.

## 10. minibeads in the suite (landed with this note)

- Submodule `ext/minibeads` (`https://github.com/rrnewton/minibeads`), pinned at v0.29.0
  (56714bf), the first release with the closed-blocker fix. `ext/` is the home for submodules,
  one directory per project, so the top level does not collect them.
- Not vendored, not built or tested here: minibeads has its own repository, CI and crates.io
  releases. `scripts/validate.py` routes the gitlink to the always-on hygiene checks only. A
  plain `git clone` of agent-utils leaves it empty, and `.gitmodules` marks it `update = none`,
  so `git submodule update --init --recursive`, here or in a project that embeds agent-utils,
  skips it (and so never fetches minibeads' own submodules, an upstream-beads fork and a
  template). `git submodule update --init --checkout ext/minibeads` fetches it on request.
- README tool table, repository layout, the website's overview and tools pages, and the setup and
  coordinator skills now list it, with the a la carte message.

## 11. Questions for the owner

1. Approve the two-tier rules in section 3, in particular "say which tier a number belongs to"
   (`GitHub #N`) and the three-day triage window.
2. B1's shape: a separate `taskctl` command, or a subcommand of an existing tool (agentctl)?
3. Should watch-agents close a task on a `DONE:` verdict by default, or only report it (the
   design defaults to report)?
4. Which account posts GitHub triage comments (the coordinator found agent-utils writes going out
   under another project's bot account)?
5. Which internal tracker should be the dispatch tier for work inside the company, if any (see the
   private note)?
