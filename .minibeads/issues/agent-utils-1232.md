---
title: 'headless-harness-adapters: run Codex and Muse headless beside Claude on one wrapper'
status: open
priority: 2
issue_type: task
assignee: mu-rsctl
created_at: 2026-11-10T17:55:00.000000000+00:00
updated_at: 2026-11-10T17:55:00.000000000+00:00
---

# Description

`agentctl run-headless` runs exactly one harness today: `headless::HARNESSES` is `["claude"]`, and the wrapper (`rs/agentctl/src/headless/wrapper.rs`) speaks Claude Code's stream-json through `headless/claude.rs` directly. The owner wants headless Codex and headless Muse too, as harness adapters next to `headless/claude.rs`, reusing everything around the protocol: the wrapper, the durable queue, the typed event log, `tail`, `console` (#670), and approvals (#671).

This epic owns the shared seam and the contract; the per-harness drivers are #1233 headless-codex and #1234 headless-muse. No code in this issue: the split is filed first so each part can be designed, gated, and landed separately.

# Design

Design input, per the owner's pointers:

- The owner's Orc diffs (harness repo `ai_docs/2026-11-05-orc-transport-stack-triage.md` section 5): D114434489 / D114379176 headless-cli run one subprocess PER TURN that resumes the on-disk session — `claude -p <prompt> --resume <id> --output-format stream-json --verbose`, and `codex exec --json` / `codex exec resume <thread-id>` (resume rejects sandbox flags; config is inherited). The long-lived stream-json "sdk" transport failed there (0/6 sendAgent, opaque turn boundaries) and was removed (D121890010). The reusable ideas are the TurnResult accepted-vs-completed contract, capability declarations, and the conformance table.
- AgentCloud's report (today): codex = `codex app-server --listen stdio://` (persistent JSON-RPC, structured turn events); muse-code = `muse serve --disable-sandbox --trust-workspace`; pi = `pi --mode rpc`. agentctl already speaks Codex app-server for goals.
- The choice per harness: a persistent protocol server (app-server / muse serve: interrupt, approvals, streaming) vs per-turn exec+resume (Orc's choice: simple, robust, no mid-turn interrupt). Suggested: app-server for Codex, muse serve for Muse, exec+resume as a fallback driver where the server transport proves opaque. Each part issue weighs it for its harness; this epic owns the conformance table across claude/codex/muse so the choice is made once, in the open.

The seam: a harness-adapter trait in the wrapper covering launch argv and environment, the input line (or RPC) for one queued item, translation of harness output into the typed events (`input.queued/started/completed`, `assistant.text`, tool calls, permission prompts, turn boundaries), interrupt, session resume identity, and the transcript the handback/turnover machinery reads. `claude.rs` becomes the first implementation of that trait, unchanged in behaviour.

In scope for the epic itself:

- A TurnResult-style accepted/completed contract shared by every adapter: the harness accepted an input (queued by its protocol) vs completed the turn, so `send`/`drain` receipts mean the same thing per harness.
- Per-harness capability declarations (interrupt, approvals, streaming, effort per #1216 agentctl-effort-option, raw turns per #1223 bridge-raw-headless), reported through `agentctl capabilities` next to `claude::capabilities()`.
- The declared effort applied on EVERY launch path (owner reply 319): start, revive, takeover/handback, turnover, and unit-supervised `run-headless` must not each invent their own default.
- The conformance table across claude/codex/muse (transport, turn boundaries, interrupt, approvals, streaming, session resume, effort, raw turns), kept with the code it describes.

# Acceptance Criteria

- `agentctl capabilities` reports each headless harness with its capability declarations; the wrapper's harness-specific calls all go through the adapter trait, with `claude` as an implementation whose behaviour (events, receipts, approvals) is unchanged.
- #1233 and #1234 can each be implemented against the seam without further wrapper surgery; their conformance rows are filled in the shared table.
- The full gate passes with the seam in place before either new harness is switched on.

# Notes

Filed 2026-11-10 on an owner request (issues only; no code yet). Related: #565 headless-coordinator (the wrapper this extends), #671 headless approvals, #670 agentctl-console (the owner's seat every headless harness inherits), #1216 agentctl-effort-option, #1223 bridge-raw-headless.
