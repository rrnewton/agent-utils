---
name: wrkslots
description: Manage isolated Git worktree slots with one-agent ownership, machine-sharded state, registered liveness, owner-alive handoff, post-exit removal, and crash recovery. Use before creating, registering, adopting, inspecting, finishing, recovering, or removing a managed worktree slot.
---

# Wrkslots

Treat the configured worktrees directory as opaque. Use `worktrees/wrkslots`; never edit
`ACTIVE.*`, `ARCHIVED.*`, or recovery journals, and never substitute raw `git worktree` or directory
removal for a lifecycle command.

Initialize from the project root with the registered liveness command. Its exit status is part of
the removal boundary: rc 0 means verified dead, rc 1 means alive, and rc 2 means unverifiable.
`--max-active-slots N` caps how many active slots this machine may hold; the cap is enforced when
an allocation is requested, and omitting it means uncapped.

```sh
wrkslots init . \
  --liveness-command ci-hub/health/agent_liveness_probe.py
```

Create one fresh slot for one agent. Repeat `--repo` and `--branch` for multiple repositories. The
configured `origin` remote is used by default; use `--remote NAME=REMOTE` to select another
already-configured remote. Callers never pass a remote URL. Wrkslots records the configured remote
identity and requires it to remain unchanged. Record the coordinator process generation even when
the owner will adopt later.

```sh
worktrees/wrkslots create slot01 \
  --agent codex-1 --task task-123 --purpose "fix parser" \
  --coordinator-pid "$COORDINATOR_PID" --owner-pid "$OWNER_PID" \
  --repo product=product --branch product=codex/fix-parser
```

Use `status --all-machines` for read-only inspection. A row without its directory, a directory
without its row, duplicate cross-shard ownership, corrupt state, or an unfinished journal refuses.
Heartbeat age and TTL expiry are diagnosis only and never authorize removal.

Use `doctor` when repairing a registry that has drifted: it lists every disagreement at once, in
human or `--format json`, instead of refusing at the first. It is diagnosis only — it authorizes
nothing, is never a precondition for removal, and always exits 0, so `status`'s refusals remain the
gate.

While the owner is alive, `finish` proves clean Git state, trusted remote identity, exact containing
remote refs, landed ancestry, and path identity. It records validation, limitations, exact commits,
and the coordinator continuation. It retains the row and all physical storage.

```sh
worktrees/wrkslots finish slot01 \
  --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1 \
  --validation "make test: pass"
```

Only the recorded coordinator may run `remove`. Removal requires the recorded handoff, registered
liveness rc 0, a dead owner process generation, and independent absence checks for cwd, executable,
file descriptors, memory maps, mount references, and the recorded cgroup. rc 1, rc 2, a live or
indeterminate owner, or any unreadable proof preserves every row and path.

Use `adopt` only to bind an unbound live owner from that owner's real process ancestry. It never
replaces a bound historical owner. If a legacy owner never bound and is independently verified
absent, the recorded coordinator uses `recover-unbound-owner`; this preserves `owner: null`, records
the recovery note and validation evidence, and prepares the same post-exit removal path.

Use `recover --coordinator-pid PID` whenever a create or removal journal exists. Preserve every path
after a refusal and report the exact message. Run `worktrees/wrkslots --help` for the complete CLI.
