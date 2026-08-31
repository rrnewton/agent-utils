# Related work for wrkslots

This note compares worktree lifecycle mechanisms, not product scope. Sources were read at exact
commits on 2026-08-29 so later changes do not silently alter the comparison.

## OpenCode

OpenCode has one worktree service that creates, lists, resets, and removes worktrees. It chooses a
directory below application-managed data, creates either an `opencode/<name>` branch or a detached
checkout, registers the directory with its project store, and emits ready or failed events. Removal
first resolves the requested directory against `git worktree list --porcelain`, disposes the store
entry and file watcher, runs `git worktree remove --force`, checks whether a failed command actually
left a registration, removes the directory, and deletes the local branch.

Source: [worktree service at df35e842](https://github.com/anomalyco/opencode/blob/df35e842f59bc115bb7c0479a8e11f017d443f2c/packages/opencode/src/worktree/index.ts).

Useful similarities:

- one service owns normal creation and deletion;
- paths are generated beneath one managed root;
- removal checks Git's registry rather than trusting the requested path alone;
- failures carry typed errors and the implementation handles the case where Git reports failure
  after the worktree registration has already disappeared.

Differences relevant here:

- the service does not record an external owner process, renewal time, or time-to-live;
- removal is an explicit user operation and uses forced deletion without first publishing dirty or
  unpushed work;
- its database and emitted events describe application state, but the worktree lifecycle is not
  reconstructed from a hash-linked append-only operation history;
- it therefore solves centralized interactive workspace management, not abandoned same-host agent
  reclaim under uncertain liveness.

Wrkslots should retain OpenCode's useful single-service boundary and post-failure Git-registry
check, while keeping the stronger owner, time-to-live, salvage, and recovery evidence required for
long-running agents.

## Gastown

Gastown manages agent worktrees as polecat sandboxes. Its current `DecideWorkstate` function derives
one cleanup verdict from several facts: agent state, hook work, cleanup status, failed push or merge
request state, direct Git dirtiness, stashes, unpushed commits, active work, and merge-queue state.
Unknown or failed Git checks become `NEEDS_RECOVERY`; they do not become a clean result. A separate
session check refuses reclaim when the tmux lookup fails or the session remains live. The command
surface exposes `gt polecat check-recovery`, `gt polecat remove`, and the deliberately destructive
`gt polecat nuke --force`, whose help says that it bypasses checks and loses work.

Sources:

- [workstate decision at 649b832b](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/internal/polecat/workstate.go)
- [reclaim blockers at 649b832b](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/internal/polecat/reclaim.go)
- [polecat lifecycle patrol design at 649b832b](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/docs/design/polecat-lifecycle-patrol.md)

Gastown also has a daemon patrol that dispatches a Dog to perform periodic database cleanup from a
formula and falls back to inline execution if dispatch fails. That is the sheriff-and-dogs mechanism
relevant to this work: cleanup is recurring work that another participant can perform, rather than
a promise that the original worker will return. The documented polecat lifecycle similarly says a
later patrol detects interrupted completion and retries cleanup.

Useful similarities:

- cleanup combines several independent state and Git facts;
- unknown Git or session state blocks ordinary deletion;
- cleanup is a recurring responsibility with another participant able to resume it;
- worktree state and agent state are presented through one derived verdict used by multiple
  callers;
- messages distinguish safe removal from work that needs recovery.

Differences relevant here:

- Gastown keeps long-lived reusable sandboxes, while wrkslots removes an expired agent slot after
  remote salvage and treats validation checkouts as disposable;
- Gastown's normal path relies partly on persisted cleanup status and merge-request workflow,
  whereas wrkslots records the exact filesystem and Git evidence needed to publish uncommitted work;
- Gastown offers a force flag that explicitly bypasses safety checks. Wrkslots does not offer that
  path for agent slots because its callers share privileges already; the useful control surface is
  a refusal that names the missing fact and remedy.

Wrkslots should retain Gastown's multi-fact derived decision and recurring helper model, while
avoiding a future action assigned to one named coordinator and preserving source state before any
ordinary agent-slot deletion.

## Other established mechanisms

Git itself stores linked-worktree registration in the repository's common directory and provides
`git worktree list --porcelain`, `repair`, `remove`, and `prune`. Those commands are necessary facts
and mechanisms, but they do not know whether an agent is alive or whether uncommitted work has been
published. Wrkslots therefore treats Git registration as one cross-check, not the ownership record.

Kubernetes uses owner references and finalizers to let controllers reconcile desired and observed
state after any individual controller restarts. Leases carry renewable timestamps, and finalizers
keep an object present until cleanup completes. The directly useful property here is not the cluster
permission model; it is that a later reconciler reads durable facts and completes pending work.

Sources:

- [Kubernetes owners and dependents](https://kubernetes.io/docs/concepts/overview/working-with-objects/owners-dependents/)
- [Kubernetes finalizers](https://kubernetes.io/docs/concepts/overview/working-with-objects/finalizers/)
- [Kubernetes Lease objects](https://kubernetes.io/docs/concepts/architecture/leases/)

Wrkslots follows that recoverable shape with local files: renewable owner evidence, append-only
operation history, and a journal view that any later participant can finish. It does not claim that
same-user local processes have separate permissions or that the files form a security boundary.
