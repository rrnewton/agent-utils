---
title: 'wrkslots-remote-proof-without-egress: apply-time ls-remote blocks recovery under egress filtering'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-18T12:30:00+00:00
updated_at: 2026-11-18T12:30:00+00:00
---

# Description

[watermelon-full-preview] Reported 2026-11-18: on a host with per-agent GitHub egress filtering, `recover-ownerless-agent-worktree ... --apply --coordinator-authorized` (74549a37) passes every pre-gate, but its apply step runs its own `git ls-remote` against GitHub and receives HTTP 403. Nothing is changed, and a 188 MB P1 removal stays on hold. The tool needs a sanctioned way to satisfy the apply-time remote-containment check without its own egress. Note that the host's `~/.gitconfig` rewrites GitHub URLs to local mirrors via `insteadOf`, so a naive local check can read a stale mirror; resolution must bypass that rewrite (`GIT_CONFIG_GLOBAL=/dev/null` or an explicit URL).

# Design

Design-first item; the coordinator approved direction is recorded in the resolution when chosen. Candidates: (a) a caller-supplied proof file (`ls-remote` output obtained through the caller's own egress path, naming the remote URL, the commit, the refs and a capture timestamp), accepted only if fresh (under 10 minutes), for the exact URL resolved without user gitconfig rewrites, and containing the needed commit; the tool records that it used a proof in the journal's existing free-form slot, not a new persisted record field. (b) A configured verification relay command (such as `remote_proof_command`) that the tool runs. (c) A local mirror with a proven freshness bound.

# Acceptance Criteria

- An apply whose own `ls-remote` has no egress completes with a fresh, exact-URL proof containing the needed commit, in a deterministic end-to-end test through the real CLI (both editions where the surface exists).
- A stale proof, a proof for a different URL (including one resolved through an `insteadOf` mirror), or one missing the needed commit refuses before any mutation.
- No new persisted record field is introduced; any unavoidable one is reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism).

# Notes

Queued after #1868 wrkslots-finish-without-handoff-dead-end, #1869 wrkslots-adopt-reports-success-binds-nothing and #1870 wrkslots-transfer-owner-held-dead-owner. Design proposed to the coordinator before any code.
