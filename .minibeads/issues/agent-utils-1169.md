---
title: 'slot-registry-discovery: agentctl finds the enclosing harness registry from inside a slot'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T23:12:17.358653878+00:00
updated_at: 2026-11-09T23:12:17.358653878+00:00
---

# Description

## Bug

From inside a wrkslots slot directory (for example /home/newton/work/dev-agent-utils/worktrees/slots/<slot>, a btrfs subvolume bind-mounted under the harness root, and itself a linked git worktree), a bare agentctl command without --registry cannot find the harness registry at /home/newton/work/dev-agent-utils/.agentctl. list silently reads an empty local registry and status fails with: agentctl: cannot inspect agent registry <slot>/.agentctl: No such file or directory. The lane had to pass --registry explicitly.

## Why

agentctl's global registry defaults to the relative path .agentctl and the main command path uses it verbatim, resolved against the working directory. Only the chat bridge lookup (chat_lookup::find_registry) walks ancestors to the nearest .agentctl; ordinary commands (list, status, send, ...) never do. A git-root based lookup would not help either: the slot is its own worktree, so the git toplevel is the slot itself. The ancestor walk is lexical, so it crosses the slot's mount and worktree boundaries without difficulty; it is simply not applied.

## Fix

Resolve the registry with a clear precedence: an explicit --registry wins; then AGENTCTL_REGISTRY when set and nonempty; then, when the value is the unset default .agentctl, walk up parent directories from the working directory to the nearest existing .agentctl (the same walk find_registry already performs), falling back to .agentctl when none exists. Apply the resolution once at the CLI entry so every subcommand sees it, in both the Rust and Python editions where the registry default exists.

## Acceptance Criteria

- From a nested directory under a harness (including a nested git worktree), agentctl list/status/send without --registry resolve the enclosing harness registry.
- An explicit --registry still wins over both the environment and discovery.
- AGENTCTL_REGISTRY, when set, wins over discovery.
- With no .agentctl in any ancestor, the default .agentctl behaviour is unchanged.
- Focused tests cover the parent walk, the nested-worktree case, env-over-discovery, and explicit-over-env precedence.

# Acceptance Criteria

See description.
