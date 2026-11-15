---
title: 'harness-support-matrix: one table of what agentctl does per harness, kept current by rule'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-15T12:30:00.000000000+00:00
updated_at: 2026-11-15T12:30:00.000000000+00:00
---

# Description

Owner 082 (2026-11-15): "I want to be extremely careful of documenting
ANY divergent support levels between different TUIs. Maybe we should
have a spreadsheet/table where we track what we do for claude/codex/muse.
I had no idea the enter-key spamming was specific to claude."

agentctl's per-harness support diverges in many places — send and
follow-up policies, the stuck-paste/Enter backstop, composer reading,
form answering, approvals, headless mode, transcripts, session identity,
restore, effort levels — and until now the only way to learn a cell was
to read the adapters. Build a harness support matrix document in
agent-utils from the current code, honestly: every per-harness branch in
`rs/agentctl` (and the Python edition where it still ships), what each
harness actually gets, where docs or skills claim more than the code
does, and the cells where the owner would be surprised. Add the rule —
in the matrix doc, in agent-utils' own AGENTS.md, and in the agentctl
skill — that any change adding or removing support for one harness
updates the matrix in the same commit. Link it from the coordinator
skill in agent-utils; the harness repo's top AGENTS.md line goes to the
coordinator to add there.

Coordinate: mu-genbackstop (#1560) is building a harness-agnostic
stuck-paste backstop, and mu-codexfooter (#1553) is fixing the Codex
footer misread; record their rows as in progress with today's truth,
and whichever lands later updates the matrix in its own commit.
mu-musehl is building headless Muse (#1234): record headless Muse as
in progress.

# Design

- `common/docs/agentctl/harness-support-matrix.md`: rows are
  capabilities (send typing/submit and receipts, follow-up while
  working, stuck-paste/Enter backstop, composer/draft detection,
  busy/idle, interrupt, approvals relay to chat, forms/questions, trust
  dialogs, headless mode, transcript, tail/console/read, session
  identity, takeover/handback, restore/revive, effort, model,
  launch-env replay, chat-bridge delivery, watcher notices, raw
  messages, goal commands, skill installation, transient recovery,
  stalled-goal resume); columns are Claude Code, Codex, Muse, Unknown
  TUI, and Headless. Cells are supported / partial / none / in
  progress, with per-row notes naming the issue and the code path.
- First version from the current code only; false claims in docs and
  skills are listed at the end, not silently corrected.
- The update rule goes in the matrix doc, repo `AGENTS.md`, the
  agentctl skill (`skills/agentctl/SKILL.md`, the source its package
  copies link to), and a link in `skills/agent-utils-coordinator/`.

# Acceptance Criteria

- The matrix doc exists on main, with every row above filled from code
  evidence (adapter overrides and every `harness ==` branch surveyed in
  both editions).
- The update rule is present in the matrix doc, `AGENTS.md`, and the
  agentctl skill; the coordinator skill links the matrix.
- The #1553, #1560, and #1234 rows read "in progress" with today's
  truth recorded.
- Docs/skills claims the code does not back are listed at the end of
  the matrix.
- `make validate` (via the gate) passes on the landing commit.

# Notes

Resolution: landed as `common/docs/agentctl/harness-support-matrix.md`
with the rule in `AGENTS.md`, `skills/agentctl/SKILL.md`, and the link
in `skills/agent-utils-coordinator/SKILL.md`. Findings worth the
owner's eye: the stuck-paste backstop is not harness-agnostic yet
(Unknown TUIs get a report, not a rescue; #1560 generalises it); no TUI
approval is relayed to chat (headless Claude only); `agentctl answer`
never answers Muse forms; Rust headless is Claude-only (Muse in
progress, #1234); effort level sets differ per harness. The exact
AGENTS.md line for the harness repo was sent to the coordinator
(coordb) to add there, per its instruction.
