# agentctl: rename, routing-identity checks, and revive (design, 2026-10-08)

Status: proposal for review. Build order, set by the owner: (1) `rename` and the
routing-identity checks, including `doctor`; (2) `revive`. Two small start
defects are fixed in their own commits (section 6).

## 0. Facts this design rests on (measured, not assumed)

- **Engine.** `common/bin/engine-resolver` runs the Python edition unless
  `DAGRUN_ENGINE=rust`. `cross/agentctl_differential.py` pins one shared
  capability list for both editions, so a shared verb needs both editions or an
  explicit edition-specific advertisement (as `move` and `agentcloud` already are).
- **Mutable state agentctl keeps** (besides the declarative `profiles.json`):
  `.agentctl/<name>/agent.json` (mutable: lifecycle, pane, goal fields),
  `.agentctl/<name>/queue/` (durable delivery queue; `target.json` binds to a
  *pane id*, not a name), `.agentctl/.<name>.lock`, `.identity.lock`, move
  intents, and `.agentctl/archive/<name>-<token>/` written once by `stop`.
  The archive is already agentctl's memory; this design makes it the source of
  truth for past agents and their conversation IDs.
- **Herdr identity is not positional.** `tab_id` (`wJ:t85`) and `pane_id`
  (`wJ:p85`) are opaque and stable; the displayed tab `number` is the decimal
  value of the base-32 suffix (`t85` = 8*32+5 = 261), with gaps where tabs
  closed. `terminal_id` (`term_65d5...`) is a second stable key. What *is*
  mutable is human-facing: the **tab label** (`herdr tab rename`, or the UI) and
  the **Herdr agent name** (`herdr agent rename`; `herdr agent get NAME`
  resolves by it, and `move` recovery depends on it). Herdr pane/tab IDs are
  only unique within one server lifetime; `terminal_id` guards reuse.
- **What the per-operation check verifies today** (`agent._validate` via
  `_checked`): pane exists; harness kind; workspace label (when configured);
  cwd; Herdr-reported session when one is recorded. It does **not** check the
  tab label, the Herdr agent name, or `terminal_id`, and records do not store
  `terminal_id`.
- **Session IDs are not captured.** `session_value` is copied once at start from
  Herdr's `pane get` `agent_session`; Herdr reports none for Claude panes. In one
  consumer's registry, 0 of 37 archived Claude records and 0 live Claude records
  hold an ID; Codex IDs exist only where a caller ran `bind-session`/`goal`.
  Crucially, `resolve_target` treats a non-null `session_value` as a *routing*
  key (it scans panes for a Herdr-reported match), so an ID agentctl merely
  knows must **not** be written there.
- **A live sweep of that registry** found 2 records whose panes no longer exist,
  2 live panes whose harness had exited (pane back at a shell), and 4 tabs in the
  project workspace that no record owns. Nothing reports this today.
- `agentctl resume NAME` already exists and means "un-pause input", hence the new
  verb is `revive`.
- **wrkslots** records per slot: `agent` (a name), `task`, `purpose`, and an
  `owner` *process* identity (pid, start ticks, boot id). It does not know
  agentctl tokens. `wrkslots adopt` binds an unowned slot to a live owner.

## 1. Names and identity

- A **live name** is unique within one registry (the directory name). That is
  the only uniqueness agentctl promises for names.
- A **token** (32-hex, already in every record) identifies one agent lifetime
  forever. Archive ID = `<name>-<token>`. Past agents are addressed by token;
  names are reusable labels. Names are therefore *not* globally unique in
  history, and nothing pretends they are.
- New record fields (Python and Rust read/write the same schema):
  - `terminal_id`: from `pane get` at start/adopt/move.
  - `conversation`: `{harness, id, source, recorded_at}` where `source` is
    `assigned` (agentctl chose it at launch), `herdr` (Herdr reported it),
    `bound` (`bind-session`), `discovered` (matched from harness files at stop),
    or `missing` (with `reason`). `session_value` keeps its current meaning
    (Herdr-observed routing identity) and is never filled from `conversation`.
  - `name_history`: `[{name, renamed_at}]`, appended by `rename`.
  - `slot`: `{name, project_root, path}` only when started with `--slot`
    (taken from `wrkslots shell-command` output, which is already parsed).
  - `revived_from`: archive token, set by `revive`.
  - `archived_at`: set by `stop`.

## 2. Routing-identity check (every pane-touching operation)

Define one **identity check** used by `send`, `drain`, `read`, `wait`, `goal`,
`attach`, `bind-session`, `move`, `rename`, `stop`:

1. existing checks (pane, harness, workspace, cwd, Herdr session);
2. `pane get` `terminal_id` equals the record's;
3. `pane get` `tab_id` equals the record's `tab_id`, and the tab has one pane;
4. `tab get` label equals the record name;
5. for adapter `herdr` (agentctl-started), `agent get PANE` name equals the
   record name. Adopted panes may have no Herdr agent name; when they have one
   it must match.

Any mismatch refuses the operation, names every differing field with recorded
and observed values, and points at `agentctl doctor`. It never re-resolves by
label or name ("never guess"). Cost: two extra Herdr calls, measured at 2-3 ms
each.

Levels (`--identity-check`, env `AGENTCTL_IDENTITY_CHECK`, or `profiles.json`
`"identity_check"`; precedence in that order):

- `standard` (default): the check before each operation.
- `paranoid`: also after each operation (after the submit for `send`/`drain`).
  A post-check mismatch cannot unsend text; it exits 76 ("delivered, identity
  changed during delivery") and records the message as `possibly_misrouted` in
  the queue's processed record.
- There is no "off" level.

Legacy records without `terminal_id` are backfilled on the first check in which
every other field matches; the result reports `identity_backfilled: true`.

## 3. `agentctl doctor` (registry-vs-Herdr sweep)

Read-only by default, one call per workspace for `tab list`/`pane list` plus one
`pane get` per record (about 50 ms for 10 agents). JSON output, exit 0 when clean,
1 on any finding, 2 when Herdr is unreachable. Findings per record:
`pane-missing` (suggest `agentctl stop NAME`), `harness-exited` (pane at a
shell), `terminal-mismatch`, `tab-moved`, `label-mismatch`, `herdr-name-mismatch`,
`workspace-mismatch`, `rename-incomplete` (journal present, section 4),
`legacy-no-terminal-id`. Workspace-level findings: `label-collision` (two tabs or
a foreign tab carry a registered name), and informational `unmanaged-tab`.
`--repair-labels` restores a tab label/Herdr agent name only for records whose
pane, terminal, tab, harness and cwd all match; everything else stays manual.
The health tick runs `agentctl doctor`.

## 4. `agentctl rename OLD NEW`

For a live agent whose warm context is still useful but whose purpose changed.
Not a substitute for starting a new agent.

Preconditions (all refusals, nothing changed):
- `NEW` passes the name rule, is not a live record, has no live Herdr agent
  named `NEW`, and no tab in the workspace is labelled `NEW`;
- `OLD` passes the full identity check; adapter is `herdr` or `herdr-foreign`
  (relay/pane/agentcloud adapters refused in the first version);
- no message for `OLD` is in flight (`queue/inflight` empty), and no move intent.

Locks: `.OLD.lock` and `.NEW.lock` in sorted order, then `.identity.lock`.

Steps, each idempotent and checked before acting:
1. write `.agentctl/.rename-OLD.json` `{old, new, token, pane_id, tab_id,
   terminal_id, started_at}` (fsync file and directory);
2. `herdr agent rename PANE NEW` (adapter `herdr`, or when a name was present);
3. `herdr tab rename TAB NEW`;
4. `renameat2(RENAME_NOREPLACE)` `.agentctl/OLD` -> `.agentctl/NEW` (helper
   already used by archive), then rewrite `agent.json` with `name=NEW` and a
   `name_history` entry (temp file + rename);
5. delete the journal.

Recovery: a journal makes every command on `OLD` or `NEW` refuse with "rename
incomplete; rerun `agentctl rename OLD NEW`". Rerunning re-verifies the pane by
`pane_id`+`terminal_id` from the journal (the name and label may be either value)
and completes the remaining steps. If the pane is gone, it completes only the
registry steps and reports `herdr_steps: skipped-pane-missing`. If any observed
value is neither OLD nor NEW, it refuses. No rollback mode in the first version.

Not updated, reported as `external_references` hints: the wrkslots `agent`
field (when the record has a `slot`, the hint names the slot), chat-bridge or
cron configuration that names `OLD`. agentctl never edits another tool's state.

## 5. `agentctl revive NAME` (after 1-4 land)

Resolution: archives whose `agent.json` name or `name_history` contains `NAME`.
Exactly one -> use it. More than one -> refuse and print, per candidate, token,
created/archived times, harness, cwd, slot, conversation source and the first
line of the last delivered message, so the coordinator picks with one more call:
`--archive TOKEN` (unique prefix of 8 or more characters). `--newest` accepts
the newest by `archived_at` (directory mtime for legacy archives, labelled so).

Launch: same harness, profile/argv and model from the archived launch block
(legacy schema-1 records rebuild it from `harness`/`model`/`arguments`), with a
structured resume of `conversation.id`; new token; `revived_from` set; tab label
and Herdr name `NAME` (or `--as NEW`). Optional `--brief/--file` delivered after
the harness is ready. The archive is never modified.

Conversation capture, so revive has something to resume:
- **Claude**: `start` generates a UUID and passes `--session-id UUID`
  (refused together with raw `--session-id/--resume/--continue`, extending the
  existing selector validation). `source: assigned`.
- **Codex**: no pre-assignment exists. `stop` (and `status`, lazily) looks in
  `$CODEX_HOME/sessions` (default `~/.codex/sessions`) for rollouts whose
  `session_meta` cwd equals the record cwd and start time is after `created_at`,
  and whose first user message equals the brief agentctl delivered. Exactly one
  -> `discovered`; otherwise `missing` with the candidate count. `herdr`/`bound`
  sources win when present.
- **Muse**: Herdr reports it (`source: herdr`).
- `stop` never refuses for a missing conversation (retirement must not depend on
  metadata); it records `missing` and why. `revive` then refuses unless
  `--session ID` asserts one explicitly.

Wrkslots, opportunistic: if the record has `slot` and wrkslots is runnable,
revive reads `wrkslots status --slot S --format json` in `project_root` and
requires an active row with the same path; it reports (does not fix) an `agent`
field that differs and prints the exact `wrkslots adopt` command when the owner
process is dead. If wrkslots is not installed, revive continues when the recorded
cwd exists and reports `slot_verification: unavailable`.

Failure modes:

| Case | Behaviour |
|---|---|
| no archive / unreadable `agent.json` | refuse; list readable candidates |
| several archives | refuse with candidate table (above) |
| live record already named `NAME` | refuse; suggest `--as` |
| same conversation already live under another name | refuse (identity lock, extending `_identity_owner` to `conversation`) |
| two revives race | serialized by `.NAME.lock` + `.identity.lock` |
| slot reclaimed or path differs | refuse; `--cwd DIR` override, but for Claude only when the session file exists under the new cwd's project directory |
| another live record has the same `slot` | refuse ("two agents in one slot"); no override |
| session expired harness-side | pre-check: Claude `~/.claude/projects/<cwd-slug>/<id>.jsonl`, Codex rollout file; after launch, a pane back at its shell within the startup timeout -> `launch_failed`, tab closed, archive untouched |
| Herdr unreachable | refuse before any state change |

## 6. Start defects (own commits, with tests)

- **Composer readiness.** `submission.submit` reads the screen once and raises
  `PromptNotStaged` when no composer is recognisable, so a fresh Claude pane that
  Herdr reports ready before the composer is drawn leaves the brief pending
  (exit 75). Fix: while nothing has been typed, poll for a recognisable composer
  within the ready timeout before refusing. Test: fake terminal shows the
  composer on the third read; the brief is delivered with exit 0; a composer that
  never appears still refuses with nothing typed.
- **Profiles from a slot cwd.** `start --profile` reads only
  `<cwd>/.agentctl/profiles.json`, while workspace policy is read from the
  registry's project. Fix: `--cwd` first (unchanged behaviour), then the
  registry's project when the registry is named `.agentctl`; the error lists
  both paths. Same for `agentctl profiles`. A slot checkout never holds the
  git-ignored profile file, so only the previously failing case changes. Test:
  `--cwd SLOT` with the profile beside the registry.

## 7. Tests

Python unit tests with the existing fake Herdr, plus differential cases in
`cross/agentctl_differential.py` for every shared verb:
- identity check refuses on each changed field (terminal, tab, label, Herdr name)
  and never resolves by label; paranoid post-check reports exit 76;
  legacy backfill only when all else matches;
- doctor: each finding kind from a synthetic registry, exit codes, read-only by
  default, `--repair-labels` refuses when any non-label field differs;
- rename: success; every refusal; crash injected after each step, then rerun
  completes; pane gone mid-rename; journal blocks other commands on both names;
  queue survives with messages delivered to the same pane;
- revive: single, ambiguous, `--archive` prefix, `--as`, legacy archive, missing
  conversation, duplicate conversation, slot reclaimed, shared-slot refusal,
  session file missing, launch failure leaves the archive byte-identical;
- Claude start passes `--session-id` and records `assigned`; raw selectors
  refused; Codex discovery: unique, ambiguous, absent;
- **no wrkslots installed** (PATH without it, `AGENTCTL_WRKSLOTS_BIN` unset):
  start, send, stop, doctor, rename and revive of a record with a `slot` all
  work, revive reporting `slot_verification: unavailable`.

## 8. Open questions for review

1. Rust parity: proposed both editions for identity checks, doctor and rename in
   this branch (routing safety must hold on either engine); revive Python-first,
   advertised as an edition-specific capability until the Rust port lands.
2. Should a `label-mismatch` in `standard` refuse (proposed) or warn? Refusal
   means a human retitling a tab in the UI blocks automation until
   `doctor --repair-labels`.
