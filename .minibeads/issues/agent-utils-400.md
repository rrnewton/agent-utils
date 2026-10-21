---
title: 'slot-reclaim-deadlock: an archived herdr-relay agent and a present slot directory refuse removal forever'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- agentctl
created_at: 2026-10-21T15:13:28.213991221+00:00
updated_at: 2026-10-21T15:13:28.213991221+00:00
---

# Description

[opus 5.5, coordinator of a development harness] Five agent slots in a coordinator harness cannot be removed by any route the tools offer. Each check waits on the other, so this is a deadlock (a catch-22), not a transient refusal. Owner requirement (2026-10-21): **every situation must have a deterministic route to slot reclamation, and anything that acts like a lock must have a helping strategy so another agent can carry the protocol forward.**

## Observed (2026-10-21, one development host, a coordinator harness, layout flat, slot_representation image, sandbox root isolation)

Slots: related-work, cmd-e-done, two-column-width, channel-groups, slack-owner-via-app (generation 1 each). Each is an ext4 slot.img plus state.img under worktrees/slot-images/agent/<slot>/, loop-mounted at worktrees/slots/<slot>. Together they hold about 100G on disk.

Their agents (related-work-writer, cmd-e-fix, two-column-fix, channel-groups-fix, slack-owner-fix) were boxed Claude sessions started with `agentctl start --slot` (adapter herdr-relay, slot_isolation root). All of them were stopped with `agentctl stop`, so their records are archived under .agentctl/archive/. No process has its cwd or root under any of the slot paths, and every slot's work is published: landed on main in 461a14f8, or kept as archive/* branches on GitHub.

`wrkslots remove <slot> --coordinator-pid P --expected-generation 1` refuses each one:

    REFUSED: registered liveness authority is unverifiable: UNVERIFIABLE agent=cmd-e-fix may_replace=false
    reason=cmd-e-fix has an agentctl record whose liveness cannot be observed here: archived agentctl record
    cmd-e-fix (adapter herdr-relay): runs its agent outside a local herdr pane, so local absence is not
    evidence of death ... rc=2

Adding `--coordinator-authorized --attest-agent-stopped <agent> --attest-note ...` refuses the same way. The attestation path accepts only reason=no-agentctl-record, and an archived record is not "no record".

## The cycle

1. **wrkslots** removes only when the registered liveness command answers dead (exit 0). The harness's bin/agent-liveness execs a consuming project's liveness probe (agent_liveness_probe.py, line numbers below refer to it).
2. **The probe** answers UNVERIFIABLE for any record it cannot bind to a local Herdr pane. That covers herdr-relay as well as remote-runtime adapters, because it assumes such an agent may run elsewhere (probe lines ~896-937). It excuses an *archived* record of that kind only when the slot directory is already absent and clear (lines ~1527-1540: `scan.slot.clear and scan.slot.absent and all(b.archived ...)`).
3. **The slot directory** is absent only after wrkslots removes it, which needs step 1. Nothing else may delete it: the coordinator rules forbid removing slots by hand.

So no participant can make progress. That includes the coordinator, a later agent, and the original agent, which is gone. Repeating the same command loops forever. `wrkslots audit` and `unpushed` keep reporting these rows indefinitely.

## Why the probe's premise is wrong here

A herdr-relay agent started by `agentctl start --slot` with slot_isolation root runs **on this host**: a local relay process plus a boxed harness under it. Its liveness can be observed locally:
- the relay's pinned process identity (pid, starttime, boot id) in the record's custom_process_identity;
- any process whose cwd or root lies under the slot path, a check the probe already makes for slot questions;
- the archived record itself, written by `agentctl stop` after it tore down the runtime.

The probe treats all non-pane adapters as remote, but only the remote-runtime adapter truly is. The probe already counts name absence for a slot question when no process uses the slot (4d65b03d0ed7). That rule should also apply to archived herdr-relay records, whose pinned process is gone, while the directory still exists.

## Liveness evidence: observe, never converse (owner, 2026-10-21)

Talking to an agent (sending it a prompt, waiting for a reply) must not be part of the reclamation protocol. An agent that is wedged, mid-turn, or behind a permission prompt answers nothing, and its silence proves nothing. Liveness comes only from evidence a helper can read without the agent's cooperation:

- **Process identity, not a bare PID.** Record pid plus /proc starttime, boot id and **PID-namespace identity** (the inode of /proc/<pid>/ns/pid, plus the mount-namespace inode where it matters). A PID means something only inside the namespace that recorded it, on the boot that recorded it. A pidfd held by a long-lived supervisor is stronger still.
- **Session tail on disk.** The harness's own transcript or journal (for example the Claude session JSONL, the agentctl output.json, the relay's log) gives last-activity time and a clean-exit marker, readable from any namespace that sees the files.
- **Use of the slot path**, as the probe already checks: any process whose cwd, root or open file lies under the slot. This holds only for processes this observer can see.

**Containers and multiple PID namespaces.** Several PID namespaces (containers, `unshare` boxes, other hosts on shared storage) can see the same slot directory. An observer in namespace A that finds no process with the recorded pid has learned nothing about an owner in namespace B. So the protocol must:
- record which namespace (and host or boot) owns each lease, and accept "dead" only from an observer in that namespace, or from evidence that crosses namespaces, such as a lease heartbeat file whose staleness is judged against the recorded boot, or a clean-exit marker;
- treat "the owner's namespace no longer exists" (its init process is gone on that boot, or the boot id changed) as decisive death evidence;
- never conclude death from absence in a different namespace. When the owner's namespace cannot be observed, fall back to lease expiry plus the clean-exit marker, or to a recorded attestation. Do not refuse forever.

The model in item 2 below must include the namespace dimension: which observer may judge which owner, and what each one can see.

## Requirements

- **Deterministic reclamation route.** For every reachable state of (agentctl record: live/archived/absent) × (adapter: pane/herdr-relay/remote-runtime) × (pinned process: alive/dead/unknown) × (slot directory: present-with-users/present-idle/absent) × (heartbeat: live/expired) × (work: published/unpublished), name the command sequence that reclaims the slot or the evidence that must change first. No state may require a hand deletion, and no state may refuse forever with an unchanged answer.
- **Helping, not blocking.** Any step that behaves like a lock must let another participant finish the protocol from durable state, the way lock-free algorithms let one thread help another: removal intent, liveness evidence, salvage, unmount, unregister. A coordinator that is not the slot's creator, a later coordinator, or a watchdog must be able to drive a stuck removal to completion or to an explicit, recorded hand-off.
- **Refusals name an exit.** Every REFUSED must name an action that changes the answer. "Refresh or repair the configured liveness source" is not actionable when the source is working as designed.
- **Fail closed only on real uncertainty.** An managed remote-runtime lane on another machine is a genuine unknown, and should still get a bounded route: query the remote runtime's session state, or take a coordinator attestation recorded in the event log. Archived local relays are not unknowns.

## Proposed work

1. **Immediate unblock:** bind archived herdr-relay records to their pinned process identity. Treat "pinned process gone and no process uses the slot path" as dead even while the directory exists. Or extend `--attest-agent-stopped` to archived records whose pinned identity is proven dead, recording the basis.
2. **Model the protocol.** The record/heartbeat/liveness/salvage/remove lifecycle across wrkslots, agentctl and the probe, for example in TLA+ or a small explicit-state model. Check that every state can still reach reclaimed or handed-off (no deadlock, no livelock), that no removal ever destroys unpublished work (safety), and that every lock-like step can be carried forward by a helper (liveness under helping). Feed the counterexamples back into wrkslots tests.
3. **Regression test** for this exact cycle: an archived herdr-relay record, a slot directory present and idle, everything published, and expect remove to succeed.

## Workaround status

None was applied: by rule, these slots cannot be deleted by hand. Their content is safe off-host. related-work and cmd-e-done landed in 461a14f8, and two-column-width, channel-groups and slack-owner-via-app are on GitHub as archive/two-column-agent-fullwidth, archive/channel-provider-groups and archive/slack-owner-via-app.

Related: agent-utils-341 (wrkslots-orphan-retire, closed), a different unremovable-slot case (dead owner, never-registered agent).
