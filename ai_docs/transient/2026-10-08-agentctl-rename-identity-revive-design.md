# agentctl: rename, routing-identity checks, and revive (design v2, 2026-10-08)

Status: **Part A (rename, identity checks, doctor) is implemented** in both
editions, after two design reviews (v1 a29e16683, v2 b28e90cd8); see section C
for what was built and verified. **Part B (revive) is parked** until Part A has
landed; its open findings (5, 6, 7, 8, 10, 11, 13) are listed in section B.

## 0. Measured facts

- **Engine.** `common/bin/engine-resolver` runs Python unless
  `DAGRUN_ENGINE=rust`; the differential pins one shared capability list.
- **Mutable state.** `.agentctl/<name>/agent.json`, `.agentctl/<name>/queue/`
  (`target.json` binds to a pane id, never to a name; `.delivery.lock`,
  `.binding.lock`), `.agentctl/.<name>.lock`, `.identity.lock`, move intents,
  and `archive/<name>-<token>/` written once by `stop`.
- **Herdr identity.** `tab_id`/`pane_id` are opaque and stable within one
  server lifetime; the displayed tab `number` is the base-32 suffix in decimal
  (`t85` -> 261). `terminal_id` names the terminal. Mutable, human-facing: the
  tab label and the Herdr agent name. A `terminal_id` does not change when the
  program inside the terminal changes.
- **Herdr input has no conditional form.** `herdr api schema --json`
  (protocol 19): `pane.send_text {pane_id, text}`, `pane.send_keys {pane_id,
  keys}`, `agent.prompt {target, text, wait}`, `agent.send_keys {target, keys}`.
  No expected-terminal or expected-process field; targets do not accept a
  `terminal_id`. Herdr's source is not in this tree.
- **What is verified today** (correcting v1). Owned records (adapter `herdr`):
  `agent get NAME` must return the recorded pane (`_WorkspaceClient.pane_info`),
  then `pane get` for workspace, cwd, harness, and the Herdr session when one
  was observed; Claude's trust prompt is screen-checked. Custom-pane records
  also pin the foreground process (`custom_process_identity`); adopted records
  pin the pane shell (`foreign_shell_identity`). Not verified: `terminal_id`,
  tab label, and, for owned records, the harness process. `agent get` already
  returns `tab_id` and `terminal_id`, so checking those costs no extra call.
- **Cost today**, counted with the executable fake Herdr of the differential
  (identical in both editions): `send` 20 Herdr calls (7 `pane read`, 5 `pane
  get`, 4 `agent get`, 2 `pane list`, 1 `send-text`, 1 `send-keys`); `start`
  27-29; `stop` 15; `status` 10. Live Herdr: 2-3 ms per `pane get`/`agent get`/
  `tab get`/`process-info`, 6 ms `agent list`, 10 ms `pane list`. A live
  `agentctl status` makes 4 calls in about 200 ms wall, mostly interpreter
  start; `list` over 8 records makes 20 calls in 346 ms. Records with an
  observed session resolve by scanning every pane (`pane list` + one `pane get`
  per pane).
- **Drift observed in one consumer's live registry**: 2 records whose panes are
  gone, 2 panes whose harness exited, 4 workspace tabs owned by no record.
- **wrkslots** records per slot an agent *name* and an owner *process*. Its
  shipped probe `py/wrkslots/examples/agentctl_liveness_probe.py` looks up
  `registry/AGENT/agent.json` and `archive/AGENT-*/agent.json`; an unknown name
  is "unverifiable" (rc 2), which blocks slot removal.

## A1. Ownership classes

| Class | Adapters | agentctl owns | Identity anchors |
|---|---|---|---|
| owned | `herdr` | tab, label, Herdr name, process | name->pane, `terminal_id`, `tab_id`, label, harness process |
| custom | `herdr-pane`, `herdr-relay` | tab, label, process | as today (`custom_process_identity`) plus `terminal_id`, `tab_id`, label |
| adopted | `herdr-foreign` | registry alias only | as today (pane, cwd, harness, shell generation) plus `terminal_id` |

Adopted records keep their existing semantics: a different label and a shared
tab remain valid, and agentctl never renames or relabels a foreign runtime.

## A2. Identity check around every input effect (implemented)

Every text or key effect is verified **immediately before and immediately
after** it: the paste and every submission-key retry inside `submit_verified`,
native `agent prompt`, the custom-pane paste and Enter, and the goal-replacement
Enter (`_GuardedTerminal` / `GuardedTerminal` wrapping the pane I/O of
`_WorkspaceClient`). It runs under the existing pane lock.

1. existing checks for the class (above);
2. `terminal_id` and `tab_id` equal the record's (from `agent get NAME` for
   owned records, `pane get` otherwise);
3. owned and custom: `tab get` label equals the record name;
4. a recipient anchor: owned and adopted records need the harness process
   pinned at start, adopt or `anchor` (boot id, pid, start ticks, executable
   device/inode; it must still be the pane's foreground group leader), or an
   observed native session; custom panes keep `custom_process_identity`, relays
   their relay process. A record with no anchor refuses input
   (`agentctl anchor NAME` pins one after the operator checks the pane).

Outcomes: a failure before anything was typed leaves the message pending
(exit 75, retryable); a failure after an earlier effect of the same submission
quarantines it as possibly submitted; a failure **after** a write quarantines
it as a probable misroute (exit 76, `"probable_misroute": true` on the message
in `failed/`). Nothing is re-resolved by label or name.

**With the Herdr change (finding 1).** When `herdr status server` prints
`capabilities: input-expect`, each write also carries `--expect-terminal
<terminal_id>` and Herdr refuses it (`expectation_failed`, nothing written)
unless the pane still holds that terminal, checked on Herdr's app thread in the
same request that queues the bytes. That closes the pane-replacement window
atomically. The Herdr branch is `agentctl-input-expect` (base: tag v0.8.0, the
deployed version), commit f8123a94600ecaae9790cad6d901e972df0d2554; its
protocol number is 20 locally, which is not upstream's 20, so agentctl checks
the capability, never the number.

**Residual, stated plainly.** Without that capability Herdr writes by pane id
alone: a pane replacement between the last check and Herdr's write still
receives that one effect, which the post-check then quarantines. With it, the
remaining case is outside Herdr's pane table: the verified harness exiting or
exec-ing inside the same terminal between the check and the write. The
post-check detects it afterwards; nothing undoes a write. A bracketed paste
into a shell with bracketed-paste mode does not execute without the submission
key, and the key is sent only after a fresh check.

**Legacy records (finding 2).** Nothing is backfilled from what Herdr shows
now, because a restarted server can recreate every visible field. Records
written before this change refuse input until `agentctl anchor NAME`, an
explicit operator assertion; doctor reports them as `unanchored`.
Rename refuses an unanchored record.

## A3. No check levels

The draft's `standard`/`paranoid` levels were dropped: the check after every
effect, which `paranoid` was to add, is always on. Its cost, measured with the
differential's fake Herdr, is 3 Herdr calls per check for an owned agent
(`agent get`, `tab get`, `process-info`); 2-3 ms each against live Herdr.
`stop` and the retirement paths keep their own proofs and run no input checks.

## A4. Registry invariants and lock order

- At most one live record per `pane_id` and per `terminal_id` in a registry.
  Enforced under `.identity.lock` at start, adopt, move and rename; doctor
  reports violations. Across registries, owned agents are protected by Herdr's
  global agent-name uniqueness; adopted aliases in two registries are not, and
  doctor run per registry cannot see the other registry.
- One lock order for every operation: name locks (sorted by name), then
  `.identity.lock`, then the queue `.delivery.lock`, then `.binding.lock`, then
  the pane lock. This matches move (name, delivery, binding, pane) and start
  and bind-session (name, identity).

## A5. `agentctl rename OLD NEW`

**Record format (re-review finding 8).** The new fields live only on flat
schema-1 records, which both editions already round-trip unchanged when they
do not know a key (Python keeps `_unknown`, Rust `#[serde(flatten)] extra`), so
an older build preserves them: `terminal_id` (string or null),
`harness_identity` (the existing six-field process identity, or null), and
`name_history` (list of `{name, renamed_at, journal_id}`, at most 256, journal
ids unique 32-hex). Nested v2/v3 records are only read, never written by either
edition; `anchor` and `rename` refuse them, and they stay unanchored. Rename
journals (`.agentctl/.renames/<token>.json`, schema `agentctl-rename/v1`, 11
keys) are written and finished by either edition; the differential runs a
rename interrupted by one edition and finished by the other, both ways.

For a live agent whose warm context is still useful but whose purpose changed.

Preconditions, checked under all locks of A4 (`OLD`, `NEW`, identity, OLD's
queue delivery and binding locks, pane): `NEW` is a valid unused live name; no
Herdr agent is named `NEW`; no tab in the workspace is labelled `NEW` (owned
and custom); `OLD` passes A2; no rename journal names either name; no move
intent. Holding the delivery lock excludes this registry's drains; messages
waiting in `inbox` move with the directory and stay bound to the same pane.
Raw pane-addressed queues from other tools are unaffected, since nothing they
bind to (pane, terminal) changes.

Steps (owned and custom do all; adopted does 1, 4, 5 only):
1. journal `.agentctl/.renames/<token>.json` `{token, old, new, pane_id,
   terminal_id, tab_id, journal_id, started_at}`: temp file, fsync, rename,
   fsync directory;
2. `herdr agent rename PANE NEW` (owned);
3. `herdr tab rename TAB NEW`;
4. rewrite `OLD/agent.json` with `name=NEW` and one `name_history` entry
   `{name: OLD, renamed_at, journal_id}` (appended only if no entry carries
   this `journal_id`), fsync file and `OLD/`; then `renameat2(NOREPLACE)`
   `OLD` -> `NEW` and fsync `.agentctl/`;
5. unlink the journal, fsync `.renames/`.

Every command that loads a record first lists `.renames/` (cheap; usually
empty) and refuses any name a journal mentions: "rename incomplete; rerun
`agentctl rename OLD NEW`".

Recovery, by rerunning the same command: a scoped loader, used only with a
journal, accepts exactly these states and requires the record token to equal
the journal token:

| Directory | `name` field | Meaning |
|---|---|---|
| OLD | OLD | before step 4 |
| OLD | NEW | inside step 4 (content published, not moved) |
| NEW | NEW | after step 4 |
| NEW | OLD, or both dirs exist, or token differs | refuse: corruption |

It re-verifies the pane by `pane_id` + `terminal_id` from the journal (label and
Herdr name may be OLD or NEW, nothing else), redoes steps 2-5 idempotently, and,
if the pane is gone, completes 4-5 and reports `herdr_steps:
skipped-pane-missing`. No rollback mode. Both editions read and write the same
journal, so either can finish the other's rename.

**wrkslots (finding 9).** agentctl does not edit wrkslots. The shipped probe is
changed to match any active or archived record whose `name` or `name_history`
contains AGENT, so a renamed agent is reported alive while it runs and dead
after `agentctl stop`. A later lifetime that reuses OLD is also matched; any
live match yields "alive", which errs toward blocking removal. Relabelling the
slot's agent name would be a new wrkslots-owned command (`adopt` refuses to
replace a historical owner); not needed for removal safety, so not in scope.
Rename reports the other references it cannot update (wrkslots slot, chat or
cron configuration naming OLD) as `external_references`.

## A6. `agentctl doctor`

Read-only by default and lock-free; its report is a labelled snapshot. One
`tab list`, `pane list` and `agent list` per workspace, then one `pane get` and
one `process-info` per record: about 20 ms + 5 ms per record of Herdr time.
Exit 0 clean, 1 findings, 2 Herdr unreachable. Per record: `pane-missing`,
`harness-exited`, `harness-replaced`, `terminal-mismatch`, `tab-moved`,
`label-mismatch`, `herdr-name-mismatch`, `workspace-mismatch`,
`rename-incomplete`, `unanchored`. Registry: `duplicate-claim` (two
records, one pane or terminal). Workspace: `label-collision`, informational
`unmanaged-tab`.

`--repair-labels` acts per owned or custom record under that record's A4 locks,
refuses while any rename journal exists or when the record is in a
`duplicate-claim`, rechecks everything under the locks, and restores the label
and Herdr name only when every other anchor matches. Adopted records are never
repaired.

Scheduling: agentctl does not schedule itself. Standalone users run `agentctl
doctor` by hand or from cron; a coordinator's health tick (for example a
tick-hub check) runs it on its own cadence. The user guide shows both.

## A7. Tests (both editions; differential cases for shared behaviour)

- Input safety: the fake Herdr swaps the pane's terminal, or its foreground
  process, immediately before each paste and each key; the replacement
  receives nothing and the refusal names the field. Harness exits between
  paste and key: no key is sent.
- Legacy: after a simulated restart that recreates labels, cwd and harness, no
  backfill occurs and doctor reports `unanchored`; adopted shell-pinned
  backfill succeeds.
- Classes: adopted alias with a different label and a shared tab keeps working
  and is never relabelled; custom adapters keep their process checks.
- Rename: success; each precondition refusal; crash injected after each step and
  inside step 4 (between content and move); fsync failure; journal present
  blocks commands on both names; recovery inserts history once when rerun
  twice; Python crash finished by Rust and vice versa; pane gone mid-rename;
  queued inbox message delivered to the same pane afterwards.
- Concurrency (barriers in the fake): rename vs send, drain, stop, move, a
  second rename, and doctor `--repair-labels`; duplicate claims refused at
  start/adopt/move/rename.
- Retirement paths unchanged: missing-pane stop, returned-shell stop,
  interrupted-move recovery.
- wrkslots probe: alive while renamed agent runs; dead after stop; reused OLD
  name by another live lifetime yields alive.
- Exit-harness (A8): adopted Claude with and without the exit confirmation;
  unknown dialog refuses with the agent still registered; running turn
  refuses without `--interrupt-turn`; timeout leaves registry unchanged;
  `--close-pane` closes only the proven idle shell; owned graceful exit; the
  archive records `retired_by` and the dialog text.
- Cost: assert call counts per operation from the fake; `send` grows by at most
  the A2 calls per effect.
- **No wrkslots installed**: start, send, rename, doctor and stop work with
  wrkslots absent from PATH and `AGENTCTL_WRKSLOTS_BIN` unset.

## A8. Retiring a harness agentctl does not own (proposed, not built)

Gap seen in use: an agent that was never registered could not be closed
through agentctl, and `stop` on an adopted record deliberately only
unregisters it (`subagents.py`: "never close, rename, signal, or otherwise
mutate the adopted runtime"). The operator typed `/exit` into the pane and
confirmed the harness's "Exit and stop tasks" dialog by hand, leaving no record.

Proposal: `agentctl stop NAME --exit-harness`, an explicit opt-in that is the
confirmation for mutating a runtime agentctl does not own.

1. Unregistered pane: `agentctl adopt NAME --pane P ...` first, so the archive
   has a record; then step 2. Two commands, both recorded.
2. Under the stop locks: A2 identity check; refuse a paused record, a non-empty
   composer, or a running turn unless `--interrupt-turn`.
3. Submit the harness's exit command through the verified submission path
   (Claude `/exit`, Codex `/quit`; the exact command and its confirmation
   screens get a screen model per harness, verified in the implementation;
   other harnesses refuse).
4. If the harness shows its exit confirmation, accept only the exact
   recognised dialog and its exit choice; any other screen refuses and leaves
   the agent running and registered.
5. Wait (bounded, `--exit-timeout`, default 30 s) until the pane returns to the
   recorded shell generation (`foreign_shell_identity`, the existing returned-
   shell proof); then archive as today with `retired_by: exit-harness`, the
   final snapshot, and the exit dialog text if one was confirmed. The pane and
   tab stay open unless `--close-pane`, which closes only a pane proven to be at
   that idle shell.

For owned records the same flag exits the harness gracefully before the tab is
closed, so the harness can finish writing its own session files (useful for
revive). Timeout or refusal changes no registry state; the result says which
step stopped.

## A9. Start defects (landed on main as 8cd29f81b and 3184726cb)

`8cd29f81b` waits up to 15 s for a recognisable composer before typing.
`3184726cb` finds profiles beside a registry named `.agentctl` when `--cwd`
has none (`--cwd` still first).

## B. Revive (parked until Part A lands)

The v1 proposal (a29e16683, section 5) stands as a starting point: archives
addressed by token, refusal on ambiguous names, Claude conversation IDs assigned
at launch with `--session-id`, opportunistic wrkslots checks. Open findings to
resolve in revive v2:

- 5: status-time capture must reload under the name lock and verify token and
  directory generation before writing.
- 6: build on the existing nested `native_session` (observed/asserted) instead of
  a new field; define migration and Rust support for nested records; reject
  conflicting IDs.
- 7: Codex rollout matching can pick one wrong session; preserve known resume
  IDs, tie a session to the process, record the session-store location.
- 8: prove the resumed session positively before any brief; define argv
  transformation (`--session-id` vs `--resume`) and the adapter matrix.
- 10: slot revalidation must use generation, storage findings, checkout identity
  and owner state, with a wrkslots-side synchronization protocol.
- 11: Rust tokens are `<nanoseconds>-<pid>`, not 32-hex; define token formats,
  short-token and prefix selection, and `--newest` ties.
- 13: define behaviour for every wrkslots failure, not only absence, and whether
  revive keeps slot boxing.

## C. What was built and how it was verified (2026-10-08)

Both editions, one commit on the agent-utils branch:
- `_GuardedTerminal` / `GuardedTerminal` around every input effect; anchors at
  start and adopt; `anchor`, `rename`, `doctor [--repair-labels]`; duplicate
  pane/terminal refusal; terminal re-pinned after a verified move.
- History-aware wrkslots liveness probe (finding 9, including the false-dead
  case: a stopped OLD archive plus a live NEW renamed from OLD).
- The differential fake Herdr gained `tab get/list/rename`, `agent
  list/rename`, `status server` with an optional `input-expect` capability,
  `--expect-terminal`, and a real process standing in for each harness.

Evidence (this host): Python 2318 related tests plus 25 new identity tests and
7 new probe tests; Rust agentctl 873 lib tests (843 before); the
Python-vs-Rust agentctl differential 376 paired checks, 0 divergences,
including a rename interrupted by one edition and finished by the other, both
ways. Herdr: 3198 of 3201 tests pass with 16 jobs; the same 3 fail on
unmodified v0.8.0.

Not built or not proven: `--exit-harness` (A8); revive (B); no end-to-end run
of agentctl against a server built from the Herdr branch, because that needs a
second Herdr server on a shared host; no test drives a full verified
submission with key retries through the guard against a scripted composer
(each retry goes through the guarded send, which unit tests cover).

## D. Handoff (findings and dead ends)

- Herdr's tab `number` is the base-32 tab id in decimal, not a position;
  routing ids are stable, labels and Herdr agent names are not.
- Herdr reports no `agent_session` for Claude panes, so agentctl never had a
  Claude conversation id; revive needs `--session-id` assigned at launch.
- The owner's Herdr fork was at 0.7.3 while 0.8.0 runs here; the `expect`
  branch is based on upstream tag v0.8.0. Upstream master is at protocol 22, so
  the fork-local 20 must be gated by the `input-expect` capability.
- Herdr does not reject unknown request fields; never send `expect` to a
  server that does not advertise the capability.
- Deploying: records started or adopted before anchors refuse input until
  `agentctl anchor NAME`; do that per agent after looking at each pane.
- Dead end: per-operation check levels; the post-effect check is cheap enough
  (3 Herdr calls, 2-3 ms each) to run always.
- Dead end: walking up from a slot directory to find profiles; slots live
  outside the project tree, so the registry's project is the fallback instead.
