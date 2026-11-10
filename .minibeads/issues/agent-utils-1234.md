---
title: 'headless-muse: a Muse adapter for run-headless (muse serve first, exec+resume fallback)'
status: open
priority: 2
issue_type: task
assignee: mu-rsctl
created_at: 2026-11-10T17:57:00.000000000+00:00
updated_at: 2026-11-10T17:57:00.000000000+00:00
---

# Description

Part of #1232 headless-harness-adapters. Add a Muse harness adapter for `agentctl run-headless` next to `headless/claude.rs`, so `agentctl start NAME --mode headless --harness muse` runs Muse under the same wrapper: durable queue, typed event log, `tail`, `console`, and approvals, with only the protocol translation being Muse's.

# Design

Transport choice, from the design input in #1232:

- Suggested primary: `muse serve --disable-sandbox --trust-workspace` — Muse's persistent server, the transport AgentCloud uses for muse-code. A persistent server is what makes mid-turn interrupt, approvals, and streaming possible at all.
- Fallback driver: per-turn exec+resume against Muse's on-disk session (the shape Orc's headless-cli chose for claude/codex: one subprocess per turn, resume by session id), where the server transport proves opaque. Simple and robust, no mid-turn interrupt; keep it behind the same adapter trait.

Muse is also the interactive TUI harness whose pane behaviour the rest of agentctl already knows (`harness/muse.rs`, the Muse transcript and session shapes in the registry). The headless adapter must agree with those: the session id a headless Muse runs is the same kind of identity `takeover`/`handback` convert between, so a Muse agent can cross between headless and interactive presentation without losing its conversation.

The adapter declares its capabilities honestly per transport: interrupt and approvals only where the server provides them, streaming where events are structured, effort (#1216 — Muse's reasoning effort is a launch/profile concern today) applied on every launch path, raw turns (#1223) delivered verbatim.

Conformance (to fill in against the epic's table): turn acceptance vs completion under muse serve; interrupt mid-turn and mid-tool; permission prompts and who may answer them; usage/cost reporting on a completed turn; server crash detection and session resume.

# Acceptance Criteria

- `agentctl start NAME --mode headless --harness muse` runs a Muse whose queue, event log, `tail`, `console`, and `status` behave as a Claude headless agent's do; `send` receipts use the shared accepted/completed contract.
- The capability declaration for Muse appears in `agentctl capabilities` and matches what the transport actually does.
- A scripted fake Muse (serve protocol or exec, matching the chosen transport) drives the wrapper conformance tests, as the fake Claude does today; the epic's conformance table gains its Muse row from observed behaviour.
- A headless Muse's session identity round-trips with the interactive Muse record shapes (takeover/handback do not strand it).

# Notes

Filed 2026-11-10 with #1232 (issues only; no code yet). Related: #1216 agentctl-effort-option, #1223 bridge-raw-headless, #670 agentctl-console.
