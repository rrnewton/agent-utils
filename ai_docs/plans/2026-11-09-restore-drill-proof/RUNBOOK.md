# Live restore drill without a reboot (#1133 restore-drill-proof)

Operator runbook for rehearsing the single host restore command against the
real registry, with no reboot and no terminal-server restart. This document
ships the procedure only; running it is a separate operator act. The
reboot-free automated proof against fully scratch state is
`py/tests/test_agentctl_restore_drill.py`; run that first and do not start
this drill while it is red.

## Scope: our registry only

The drill touches exactly one registry, supplied by the operator as the
single `--registry` value. For this host's own harness that value is:

    OUR_REGISTRY=/home/newton/work/dev-agent-utils/.agentctl

Every other team on the host is out of scope. This runbook calls the
nearest one `consumer-a`; the operator supplies its registry path
read-only for the before/after inventory and never passes it to a restore
invocation:

    CONSUMER_A_REGISTRY=<operator-supplied path, read-only inventory only>

Our workspace id is likewise operator-verified before the drill (at
writing time our lanes live in workspace `wV`; verify, never assume):

    OUR_WORKSPACE=<operator-verified workspace id, e.g. wV>

Hard scope lines. The drill performs none of these, and the pre-check
below proves the planned invocation cannot either:

- No terminal-server restart, stop, or reconfigure. The server stays up;
  the simulated reboot kills only our harness processes.
- No bridge, watcher, or unit action beyond what our own registry's
  restore itself performs (our configured units only, inventoried below).
- Nothing under `~/.local/bin`.
- No write to consumer-a's registry, panes, tabs, or queues; no tab close
  outside our workspace.
- No agent-teams state read or written.

## Approval gate

Do not start until all three hold:

1. The owner has explicitly approved a live drill in a named window.
2. The window is quiet: our lanes are idle or parked, every lane has
   written a restart note (background jobs, monitors, subagents and crons
   to re-arm do not survive), and no lane holds input it cannot lose.
3. The coordinator runs the drill personally, from outside the set of
   lanes it will kill, and stays present for the read-back.

## Pre-check: prove scope before anything is killed

1. Inventory our agents and harness pids. Save the output; every pid the
   drill later signals comes from this inventory and is re-checked
   immediately before the signal.

       agentctl --registry "$OUR_REGISTRY" list
       agentctl --registry "$OUR_REGISTRY" status <each-name>

2. Inventory consumer-a the same way, read-only (`list`/`status` only).
   Save names and pids; the closing proof compares them.

       agentctl --registry "$CONSUMER_A_REGISTRY" list

   **Abort** if either inventory is unreadable, malformed, or names an
   agent the operator cannot account for. An inventory that cannot be
   read cannot prove scope afterwards.

3. Read our `restore.json` and write down, verbatim: `coordinator`,
   `units`, `also_registered`, `exclude`, `certificate_file`,
   `herdr_unit`. **Abort** if any `units` entry is shared with
   consumer-a or is not ours to start; if any `also_registered` registry
   is consumer-a's or outside our own harness tree (restore rewrites the
   coordinator's pane there); or if `herdr_unit` names anything the drill
   would need to start (the server is already up, so nothing may start
   it). The planned command may start only the inventoried units.

4. Confirm the invocation shape. The drill runs exactly one of these,
   naming only our registry -- never a teams file that lists more:

       agentctl restore-teams --registry "$OUR_REGISTRY" --force
       agentctl --registry "$OUR_REGISTRY" restore --force

   Prefer the first: it is the boot command, confined by its single
   `--registry` filter to our records, panes and reports. If anything in
   steps 1-3 suggests the teams path would act wider, use the
   per-registry `restore` instead; it reads only our registry by
   construction. Run either once with `--dry-run` first and read the
   plan: every row must name only our agents.

5. Confirm tab scope: list the panes in our workspace and confirm every
   tab restore could label or close (idle bare shells naming our lanes)
   belongs to `$OUR_WORKSPACE`. Restore's tab hygiene never touches
   registered panes or running programs, but **abort** if any candidate
   bare shell sits in a workspace the operator cannot verify as ours.

## Simulated reboot: our agents only

1. End every one of our lanes' turns. From the saved inventory, take each
   harness pid literally -- never a pattern, never a process group. For
   each pid, immediately before signalling, re-check it:

       ps -o pid,comm -p <pid>

   The command name must still be that lane's harness. Signal only on an
   exact match; a mismatch or a vanished pid is an abort for the whole
   drill (the inventory is stale). Do not signal the coordinator process
   running this drill, any consumer-a pid, or the terminal server.

2. Leave the terminal server running. Our panes keep their shells; that
   is the within-boot shape of the drill. (Server-restart layout
   restoration and previous-boot records are what the scratch drill and
   the Rust scripted-host suites prove; a live drill must not restart a
   server other teams are using.)

3. Boot identity: the kernel boot id does not change without a reboot and
   the product offers no override. The drill therefore runs the
   per-registry invocation with `--force` under the current boot, which
   re-executes instead of replaying this boot's recorded report. Do not
   edit records or reports to fake a boot change on the live registry;
   previous-boot handling is covered off-host (see step 2).

4. If our `restore.json` configures `certificate_file`, confirm the file
   is present before running; the drill does not rehearse the
   certificate wait against the live registry (the scratch drill does).

5. Run the single approved invocation from pre-check step 4. Capture
   stdout (the JSON report), stderr (the per-agent table) and the exit
   code. Expected: 0, or 74 with every refused lane named and explained.
   75 means incomplete: stop and treat it as a failed drill (below).

## Abort conditions

Stop the drill and do not run the restore if any of these occurs:

- The owner approval or quiet-window conditions lapse.
- Either inventory cannot be read, or changes between inventory and kill.
- A pid re-check (`ps -o pid,comm`) disagrees with the inventory.
- The dry-run plan names any agent, unit, registry, or workspace outside
  the inventoried scope.
- The restore command would need a server restart, a unit outside our
  inventoried list, or any path under `~/.local/bin`.
- Consumer-a activity is observed that the drill could disturb (a lane
  mid-turn in a shared workspace, a shared unit changing state).

After the kill step, "abort" means: run no further kills, revive what was
killed with the approved per-registry invocation, and declare the drill
failed in the read-back.

## Read-back and verdict

1. Ours: print the per-agent table and open the JSON report (its path is
   in the report's `report` field and under our `report_dir`). Confirm:
   every killed lane is `revived` (or named `refused` with a reason the
   operator accepts), dispositions are sane, slot lanes show
   `slot_owner` transferred to the new harness pid (cross-check with
   `wrkslots status` on our project), each revived lane has exactly one
   queued resume brief/catch-up for this boot, and each lane answers on
   its own name. Re-arm background jobs, monitors and crons from the
   restart notes.

2. Consumer-a closing proof: repeat the read-only inventory and compare
   pids name by name against the pre-check save. **Any** difference -- a
   changed pid, a missing or new agent, a closed tab in their workspace
   -- is a **failed verdict for the drill**, however green our own table
   looks. Record both inventories verbatim in the drill report.

3. Verdict, recorded by the coordinator: `passed` only when our exit
   class is 0 (or an accepted, explained 74), every our-lane check in
   step 1 holds, and the consumer-a inventories are identical. Anything
   else is `failed` with the first divergent fact named. A failed drill
   is a finding against the restore path, not a reason to rerun until
   green.

## What this drill does not prove

It does not prove previous-boot record handling, server-restart layout
restoration, certificate timing, or slot-image remount after a real
reboot; the scratch drill (`py/tests/test_agentctl_restore_drill.py`)
covers those against disposable state, and the boot units themselves are
checked read-only with `systemd-analyze --user verify` (see
`ai_docs/plans/2026-11-06-restore-in-place/DRILL.md`). A real reboot
drill remains a separate, separately approved act.
