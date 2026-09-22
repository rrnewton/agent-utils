# Related work for wrkslots

This note compares systems that create, own, recover, and retire working copies. It focuses on
lifecycle guarantees rather than command syntax or product scope. The OpenCode and Gastown sources
were read at their pinned revisions on 2026-08-29; the other GitHub-hosted sources were checked at
their pinned revisions on 2026-09-22. The Kubernetes website citations are live documentation
rather than immutable sources.

## Bottom line

The ecosystem does not lack worktree managers. It lacks a harness-neutral recovery controller for
authored work on a long-lived shared host.

- Git and standalone managers understand repositories but not the exact process that owns a slot.
- Agent products understand their own sessions but do not expose a shared ownership and recovery
  protocol to other harnesses.
- Ephemeral CI deregisters one-job runners; separate operator automation can export logs and wipe
  or replace their machines.
- Kubernetes supplies a useful reconciliation model, but not Git, process, or salvage semantics.

Wrkslots occupies the intersection: exact renewable owner evidence, conservative host-wide
liveness checks, distinct policies for authored and disposable work, verified remote salvage,
path and generation fencing, and crash-resumable cleanup evidence. It should reuse Git and proven
worktree-manager mechanics and own only this cross-harness evidence and recovery layer.

### Current implementation and migration boundary

The migration is staged. Python `wrkslots` remains the lifecycle authority. The current Rust
`wrkslotsd` component is only a read-only observer of the hash-linked event log and a builder of a
disposable SQLite index. It has no daemon or socket behavior and performs no lease management,
cleanup, rescue, deletion, or repository mutation. The reconciliation daemon and lifecycle actions
discussed below are roadmap, not behavior shipped in the current Rust component. Its fail-closed
input boundary accepts at most 16 MiB per event and 127 nested JSON containers, Unicode scalar
strings, exact signed/unsigned 64-bit integers, and finite binary64 floats; Python remains
authoritative for its broader JSON input domain. Rust also rejects unknown event kinds, non-array
imported holds, and non-object state evidence even where the current Python reader ignores those
values; the Python writer emits the stricter shape. The CLI renders active and archive revisions as
decimal JSON strings so all `u64` values remain exact. First index publication requires a filesystem
that supports Linux `renameat2(RENAME_NOREPLACE)`.

## Capability comparison

| System | Useful capability | Gap relative to wrkslots |
| --- | --- | --- |
| Git | Worktree registry, locks, repair, clean removal, and pruning registrations whose directories are already gone | No agent ownership or liveness, dirty-work salvage, reconciliation daemon, or crash journal |
| Worktrunk | Age-gated pruning of clean integrated worktrees, dirty and locked refusal, process reaping, trash staging, and process-group tethering | Command-driven rather than lease-driven; no remote rescue of abandoned dirty work or resumable cleanup journal |
| WTP | Managed paths, setup hooks, branch-aware removal | Explicit removal only; force mode discards dirt; no stale-owner policy or salvage |
| git-worktree-runner | Agent launch, pull-request-aware cleanup, dirty-tree refusal, and phantom-registration repair | No exact owner identity, renewable lease, autonomous expiry, or salvage |
| Claude Code | Native agent worktrees, live-owner locks, periodic stale cleanup, unpublished-work protection, and a commit/push/draft-PR completion path | Product-specific; the core implementation is not publicly auditable and no cross-harness ownership or deterministic rescue contract is published |
| Codex | Atomic thread-owner metadata, managed roots, resumable-thread association, and clean-only removal | Ownership is explicitly not activity or exclusion; CLI automatic cleanup is disabled; no TTL, host-wide liveness proof, or salvage |
| OpenCode | Central create/list/reset/remove service and post-failure Git-registry checks | Explicit force-removal workflow; no renewable owner evidence or salvage |
| Vibe Kanban | Registered-workspace inventory, orphan cleanup, and forced Git/filesystem removal | Application-specific and destructive; no external owner lease, conservative liveness proof, or salvage-before-delete contract |
| Gastown | Multi-fact recovery verdict, live tmux checks, and recurring patrol | Coupled to reusable polecat sessions; no deterministic remote salvage or journaled filesystem evidence |
| Parallel Code | Task/worktree/agent association, agent termination before close, and dirty/unmerged warnings | Confirmed close ultimately force-removes, including a recursive-deletion fallback; no expiry or rescue protocol |
| jj workspaces | Snapshot tracked working-copy changes before removal and recover stale working copies through the operation log | Ignored files are deleted; no process ownership, TTL, disk-pressure controller, or remote publication |
| Ephemeral CI runners | One-job deregistration plus separately operated log export and machine replacement | Isolation depends on operator automation; deliberately loses unpublished local state and does not solve authored work on a shared host |
| Kubernetes controllers | Leases, owner references, finalizers, and restartable reconciliation | An architectural model rather than Git/process/filesystem salvage semantics or a local security boundary |

## Git: the storage substrate, not the lifecycle authority

Git stores linked-worktree registrations in the repository's common directory. `git worktree
list --porcelain`, `lock`, `repair`, `remove`, and `prune` are the necessary low-level mechanisms.
Pruning only removes administrative entries after their working directories are already missing;
it does not select or delete abandoned working directories. Clean removal protects tracked and
untracked files unless the caller supplies `--force`.

These facts are necessary but incomplete. Git cannot identify the process or session that owns a
worktree, distinguish a live owner from a recycled PID, classify a validation checkout as
disposable, or prove that dirty and unpushed work was published before removal. Wrkslots therefore
treats Git's registry as one observed fact, not as the ownership record.

Source: [Git worktree documentation at 3bc03411](https://github.com/git/git/blob/3bc0341126508f78f5869cbfc0005e987efdf0c7/Documentation/git-worktree.adoc).

## Standalone worktree managers

### Worktrunk

Worktrunk is the closest public standalone comparator. `wt step prune` considers age, integration,
dirt, and locks; its documented default minimum age is one day, and it removes only integrated,
clean, unlocked candidates. `wt remove` can reap processes associated with a worktree, and
`wt step tether` binds a process group to worktree lifetime. Removal uses trash staging where the
platform supports it.

This is a strong basis for interactive worktree hygiene, and wrkslots should track its Git-facing
mechanics instead of recreating generic worktree UX. The remaining distinction is authority:
Worktrunk's age is worktree or reference age, not a renewable lease held by an exact process
identity. Its process discovery is narrower than a conservative host-wide reconciliation across
process cwd, file descriptors, mapped files, cgroups, mounts, and namespaces. It also does not
publish abandoned dirty work to a deterministic rescue branch before reclaiming space.

Sources:

- [pruning design at 2a4aa28c](https://github.com/max-sixty/worktrunk/blob/2a4aa28c269230dbbeb8b5a64cb3dfb30c1b2fdb/docs/src/content/docs/step.md)
- [removal and process reaping at 2a4aa28c](https://github.com/max-sixty/worktrunk/blob/2a4aa28c269230dbbeb8b5a64cb3dfb30c1b2fdb/docs/src/content/docs/remove.md)
- [removal guard at 2a4aa28c](https://github.com/max-sixty/worktrunk/blob/2a4aa28c269230dbbeb8b5a64cb3dfb30c1b2fdb/src/commands/remove.rs#L252-L268)
- [prune implementation at 2a4aa28c](https://github.com/max-sixty/worktrunk/blob/2a4aa28c269230dbbeb8b5a64cb3dfb30c1b2fdb/src/commands/step/prune.rs)

### WTP

WTP creates branch-named worktrees beneath a configured root and can run setup hooks. Removal is an
explicit command. Its force option can discard dirty content, and it has no external owner lease,
stale-owner reconciliation, or salvage-before-delete contract.

Sources:

- [WTP overview at 842920d4](https://github.com/satococoa/wtp/blob/842920d489e96c67798c6b2689f48dc885e42094/README.md)
- [WTP removal at 842920d4](https://github.com/satococoa/wtp/blob/842920d489e96c67798c6b2689f48dc885e42094/cmd/wtp/remove.go)

### git-worktree-runner

git-worktree-runner launches coding agents in worktrees and includes pull-request-aware cleanup.
Its cleanup skips dirty trees by default and repairs locked phantom registrations. This directly
addresses the common launch-and-clean workflow. It does not record a generation-bound process
owner or renewable lease, run an autonomous expiry policy, or rescue unpublished work; the force
path remains destructive.

Sources:

- [cleanup at d576398f](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/lib/commands/clean.sh)
- [removal at d576398f](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/lib/commands/remove.sh)

## Agent harnesses and workbenches

### Claude Code

Claude Code exposes native `--worktree` sessions and isolated subagents. Its public changelog
documents owner locks while sessions run, periodic cleanup of stale worktrees, protection for
unpushed and untracked work, and a normal completion path that can commit, push, and open a draft
pull request.

This is substantial lifecycle behavior, but the public repository exposes changelog statements,
not the core implementation. The evidence therefore supports the documented behavior, not claims
about its exact liveness proof or atomicity. It is also a single-product facility: no public
protocol lets another harness share its ownership decisions or finish its recovery transaction.

Sources from the public changelog at `56f36532`:

- [worktree lifecycle entry at lines 3739-3742](https://github.com/anthropics/claude-code/blob/56f36532530f88b572854538d685fcf781141e8c/CHANGELOG.md#L3739-L3742)
- [worktree lifecycle entry at lines 1398-1401](https://github.com/anthropics/claude-code/blob/56f36532530f88b572854538d685fcf781141e8c/CHANGELOG.md#L1398-L1401)
- [worktree lifecycle entry at lines 3179-3183](https://github.com/anthropics/claude-code/blob/56f36532530f88b572854538d685fcf781141e8c/CHANGELOG.md#L3179-L3183)

### Codex

Codex's public Rust implementation records thread ownership atomically under a managed worktree
root and lets its browser associate worktrees with resumable or archived threads. Removal refuses
dirty worktrees. The source also explicitly warns that ownership metadata does not prove activity
or exclusive use, and its CLI configuration disables the automatic cleanup used by another
surface.

The metadata is a useful identity primitive, but not a host-wide lease: it has no published TTL,
exact process-liveness evidence, or abandoned-work salvage transaction.

Sources at `286d4ecf`:

- [thread ownership metadata](https://github.com/openai/codex/blob/286d4ecf44b4e9daba0a9fdd229a4047b770a71a/codex-rs/worktree/src/metadata.rs#L1-L85)
- [ownership caveat](https://github.com/openai/codex/blob/286d4ecf44b4e9daba0a9fdd229a4047b770a71a/codex-rs/tui/src/worktree_browser.rs#L1-L47)
- [clean-only removal](https://github.com/openai/codex/blob/286d4ecf44b4e9daba0a9fdd229a4047b770a71a/codex-rs/worktree/src/lib.rs#L284-L328)
- [CLI cleanup setting](https://github.com/openai/codex/blob/286d4ecf44b4e9daba0a9fdd229a4047b770a71a/codex-rs/worktree/src/settings.rs#L1-L36)

### OpenCode

OpenCode has one worktree service that creates, lists, resets, and removes worktrees. It chooses a
directory below application-managed data, registers the directory with its project store, emits
ready or failed events, and disposes its watcher and store entry during removal. If Git reports a
removal failure, it checks whether the registration nevertheless disappeared.

The single-service boundary and post-failure registry check are worth retaining. OpenCode does not
record an external process owner, renewal time, or TTL, and removal is an explicit force operation
without prior publication of dirty or unpushed work.

Source: [OpenCode worktree service at df35e842](https://github.com/anomalyco/opencode/blob/df35e842f59bc115bb7c0479a8e11f017d443f2c/packages/opencode/src/worktree/index.ts).

### Vibe Kanban

Vibe Kanban reconciles its registered workspaces with repository worktrees and removes orphans.
Its removal path invokes forced Git worktree removal and falls back to recursive filesystem
deletion when the directory remains. This is a direct precedent for inventory-driven cleanup, but
the destructive fallback is not a substitute for exact renewable ownership, conservative host-wide
liveness checks, or salvage-before-delete.

Sources at `d5cbb538`:

- [orphan cleanup](https://github.com/BloopAI/vibe-kanban/blob/d5cbb5380fa0b32e98ef9b8d987f63decce4be3a/crates/workspace-manager/src/workspace_manager.rs#L537-L645)
- [forced Git and filesystem cleanup](https://github.com/BloopAI/vibe-kanban/blob/d5cbb5380fa0b32e98ef9b8d987f63decce4be3a/crates/worktree-manager/src/worktree_manager.rs#L228-L260)

### Gastown

Gastown manages agent worktrees as polecat sandboxes. `DecideWorkstate` combines agent state, hook
work, cleanup status, failed pushes or merge requests, Git dirtiness, stashes, unpushed commits,
active work, and merge-queue state. Unknown or failed Git checks become `NEEDS_RECOVERY`, not
clean. A separate tmux check refuses reclaim when lookup fails or the session is live. Its recurring
patrol can retry interrupted cleanup through a later participant.

This multi-fact, fail-closed verdict and recurring reconciler are strong precedents. Gastown is
coupled to long-lived reusable polecat sessions and permits an explicitly destructive force path;
it does not define wrkslots-style verified remote salvage and journaled filesystem evidence.

Sources at `649b832b`:

- [workstate decision](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/internal/polecat/workstate.go)
- [reclaim blockers](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/internal/polecat/reclaim.go)
- [lifecycle patrol design](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/docs/design/polecat-lifecycle-patrol.md)

### Parallel Code

Parallel Code associates tasks, agents, and worktrees. Its close flow terminates the agent and warns
about dirty or unmerged work. After user confirmation it can force-remove the worktree, with direct
recursive deletion as a fallback. That is a useful interactive warning boundary, but not an
autonomous stale-owner or salvage contract.

Sources at `47ef0dec`:

- [task and worktree model](https://github.com/johannesjo/parallel-code/blob/47ef0dec7559602402ad4637053729aa1e2ac04e/README.md#L46-L77)
- [close-task orchestration](https://github.com/johannesjo/parallel-code/blob/47ef0dec7559602402ad4637053729aa1e2ac04e/electron/ipc/tasks.ts#L58-L79)
- [dirty and unmerged work warning](https://github.com/johannesjo/parallel-code/blob/47ef0dec7559602402ad4637053729aa1e2ac04e/src/components/CloseTaskDialog.tsx#L80-L159)
- [force-removal path](https://github.com/johannesjo/parallel-code/blob/47ef0dec7559602402ad4637053729aa1e2ac04e/electron/ipc/git.ts#L833-L890)

## Alternative lifecycle models

### jj workspaces

Jujutsu's working-copy model snapshots tracked changes before removing a workspace, and its
operation log supports recovery from stale working copies. This is a useful model for separating a
durable repository operation from a disposable checkout. The documented removal semantics delete
ignored files, however, and jj does not own process identity, renewal, disk-pressure policy, or
remote rescue publication.

Sources at `899c03f2`:

- [working-copy model](https://github.com/martinvonz/jj/blob/899c03f24b45f02e132121e7c118e01cc6afd19c/docs/working-copy.md)
- [snapshot-before-remove implementation](https://github.com/martinvonz/jj/blob/899c03f24b45f02e132121e7c118e01cc6afd19c/cli/src/commands/workspace/remove.rs)

### Ephemeral CI runners

GitHub's ephemeral mode automatically deregisters a runner after it processes one job; that action
does not itself export logs or wipe the host. GitHub tells operators to forward runner application
logs externally, while autoscaling or reimaging automation separately retires, wipes, or replaces
the machine. Together those practices are the strongest answer for genuinely disposable work
because cleanup does not need to infer whether a later owner will return.

The Actions runner's `PipelineDirectoryManager` is also not an unconditional between-job wipe. It
deletes known pipeline directories only when `workspace.clean` selects `all`, `resources`, or
`outputs`; otherwise it retains them.

That model is safe only after valuable output has left the worker. It deliberately does not
preserve an agent's uncommitted source, untracked experiments, or local commits that were never
published, and it does not address several long-lived agents sharing one development host.

Sources:

- [GitHub ephemeral runner guidance at 5648c698](https://github.com/github/docs/blob/5648c6985586b49f88b98e5a8b7e6d60f207b3a4/content/actions/reference/runners/self-hosted-runners.md)
- [runner workspace cleanup at 50bd7667](https://github.com/actions/runner/blob/50bd7667ef037c0bef8fdc38337a7a797f2279c7/src/Runner.Worker/PipelineDirectoryManager.cs)

### Kubernetes controllers

Kubernetes uses owner references and finalizers so a controller can reconcile desired and observed
state after any particular controller restarts. Leases carry renewable timestamps, while a
finalizer prevents an object from disappearing until cleanup has completed. These are useful
control-plane patterns, not claims that same-user local processes have separate permissions or
that a local state file is a security boundary.

Sources:

- [owners and dependents](https://kubernetes.io/docs/concepts/overview/working-with-objects/owners-dependents/)
- [finalizers](https://kubernetes.io/docs/concepts/overview/working-with-objects/finalizers/)
- [Lease objects](https://kubernetes.io/docs/concepts/architecture/leases/)

## Design lessons for wrkslots

The comparison narrows what wrkslots should implement:

1. Use Git's registry and removal operations; do not build another Git front end.
2. Borrow Worktrunk's clean/integrated/locked classification, process reaping, and trash staging.
3. Follow Gastown and Kubernetes in deriving decisions from several durable facts and letting a
   later reconciler resume an interrupted transaction.
4. Treat harness metadata as a useful input, never sufficient proof that a process is live or dead.
5. Keep authored agent slots distinct from disposable validation checkouts. Only the latter may be
   reclaimed without source salvage.
6. Make cleanup monotonic and generation-fenced: revalidate immediately before every irreversible
   step, and record enough evidence to resume rather than guess after a crash.
7. Publish dirty, untracked, and unpushed authored work to a deterministic rescue reference before
   removal; optional agent-written summaries may add context but must not change the rescue result.
8. Keep the reconciler narrow. Repository-specific policy, build-cache eviction, and session
   discovery belong behind adapters rather than in the core lifecycle state machine.
