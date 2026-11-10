---
title: 'headless-codex: a Codex adapter for run-headless (app-server first, exec+resume fallback)'
status: open
priority: 2
issue_type: task
assignee: mu-rsctl
created_at: 2026-11-10T17:56:00.000000000+00:00
updated_at: 2026-11-10T17:56:00.000000000+00:00
---

# Description

Part of #1232 headless-harness-adapters. Add a Codex harness adapter for `agentctl run-headless` next to `headless/claude.rs`, so `agentctl start NAME --mode headless --harness codex` runs Codex under the same wrapper: durable queue, typed event log, `tail`, `console`, and approvals, with only the protocol translation being Codex's.

# Design

Transport choice, from the design input in #1232:

- Suggested primary: `codex app-server --listen stdio://` — a persistent JSON-RPC server with structured turn events. agentctl already speaks Codex app-server for goals, so the client machinery and its failure modes are known. A persistent server is what makes mid-turn interrupt, approvals, and streaming possible at all.
- Fallback driver: per-turn `codex exec --json` / `codex exec resume <thread-id>` (Orc's headless-cli choice). Simple and robust, with clean per-turn process boundaries, but no mid-turn interrupt and no live approvals; resume rejects sandbox flags, and configuration is inherited from the environment. Keep this behind the same adapter trait so a transport that proves opaque (the Orc sdk transport failed 0/6 sendAgent with opaque turn boundaries and was removed) can be swapped without touching the wrapper.

The adapter declares its capabilities honestly per transport: interrupt and approvals only where the server provides them, streaming where events are structured, effort (#1216) mapped to Codex's own controls and applied on every launch path, raw turns (#1223) delivered verbatim. Session identity is the app-server thread (or the exec resume thread-id); transcript and resume semantics feed the existing handback/turnover machinery.

Conformance (to fill in against the epic's table): turn acceptance vs completion under app-server events; what an interrupt does mid-turn and mid-tool; how a permission prompt surfaces and who may answer it; what usage/cost (if any) a completed turn reports; how a crashed server is detected and resumed.

# Acceptance Criteria

- `agentctl start NAME --mode headless --harness codex` runs a Codex agent whose queue, event log, `tail`, `console`, and `status` behave as a Claude headless agent's do; `send` receipts use the shared accepted/completed contract.
- The capability declaration for Codex appears in `agentctl capabilities` and matches what the transport actually does (interrupt/approvals claimed only if demonstrated against the real server).
- A scripted fake Codex (app-server or exec, matching the chosen transport) drives the wrapper conformance tests, as the fake Claude does today; the epic's conformance table gains its Codex row from observed behaviour.

# Notes

Filed 2026-11-10 with #1232 (issues only; no code yet). Related: #1216 agentctl-effort-option (declared effort on every launch path), #1223 bridge-raw-headless (raw turns), #670 agentctl-console.
