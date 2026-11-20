---
title: 'agentctl-watch-stale-background-shell-footer: watch must read the background-shell count from the live footer only'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- watch
created_at: 2026-11-20T02:09:21-05:00
updated_at: 2026-11-20T02:09:21-05:00
---

# Description

[watermelon-full-preview] `agentctl watch-agents` keeps flagging a Muse lane with "status idle waiting on background work the screen counts (1 background shell), but no shell runs under its harness", while the lane's own background task list is EMPTY (checked at about 3:22 AM ET on 2026-11-20), and it re-flags repeatedly. The screen count and the process tree disagree, and the watch believes the screen every pass.

Likely cause: the Muse screen parser reads a stale "1 background shell" footer (or scrollback) as current. A footer line that scrolled into history is not the live status footer.

# Design

- Take the background-shell count only from the live status footer at its fixed position (the bottom rows of the pane), never from scrollback; a matching line elsewhere on screen is history, not state.
- When the screen count disagrees with the process tree (no shell runs under the pinned harness), prefer the process tree: the agent is idle, not waiting on background work.
- Report such a state at most once per state; no repeated notices for an unchanged disagreement.
- Tests use recorded pane text: a stale footer in scrollback plus a current footer with no background work gives no notice.

# Acceptance Criteria

- With recorded pane text carrying a stale "1 background shell" in scrollback and a live footer with no background work, watch reports no background-wait notice.
- When the live footer and the process tree agree that shells run, the existing background-wait behaviour is unchanged.
- A screen/process-tree disagreement is reported at most once per state, never once per pass.
- The agentctl Rust test suite stays green; no persisted field changes (reader-skew check: none expected; report any new persisted field before gating).

# Notes

Reported by coordb 2026-11-20; the consumer lane is not named (repository boundary rule). Lane mu-agentctlfix, after #1937 agentctl-status-redact-argv and #1938 agentctl-pidfd-open-einval are gated. Suspected parser surface: the Claude footer counting in `rs/agentctl/src/harness/claude.rs` and the background-wait classification in `rs/agentctl/src/agent_watch.rs`.
