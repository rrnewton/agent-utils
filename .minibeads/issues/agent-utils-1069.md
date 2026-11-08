---
title: 's2s-bridge-tool-verification: prove the private voice bridge attaches the document tools'
status: open
priority: 3
issue_type: task
assignee: mu-docreader
created_at: 2026-11-08T15:30:42.000000000+00:00
updated_at: 2026-11-08T15:30:42.000000000+00:00
---

# Description

Residue from #1040 documents-voice-agent (closed with live numbers recorded there). The document tools (`fetch_document`, `fetch_document_section`) are proved on the deployed websocket cascade: every whole-document harness trial called `fetch_document` once, and yes-to-first-words measured 1.75 s median. Nothing in this repository proves the same for the private S2S bridge: on the `vibe-talk-v1` path the bridge is a separate deployment that must attach `/mcp` to its model and emit tool calls, and no deployment reachable during the #1040 acceptance ran it. Two smaller residues ride along: the measured yes was typed (a spoken, ASR yes adds recognition time the harness does not model), and answer wording/section citation were never directly observed because the acceptance harness is deliberately content-free.

# Design

- When a deployment serves the private S2S bridge, run one live session with a document open (owner-driven or `voice-read-acceptance.py` document mode against that stack) and confirm in the server access log that the session lists and calls the document tools; record greeting behaviour (whether the notice becomes the first turn or waits behind a greeting) and whether the bridge fetches the voice-agent profile per session.
- If the bridge cannot tool-call, fall back to page-driven notice prompts carrying sections (no tool support needed), sized from the same notice caps.
- Add one spoken-yes measurement (owner's voice, manual number kept separate from the typed-T0 numbers) and one content-checked answer (owner listens or a transcript-keeping local run) to confirm the answer cites the section its facts came from.

# Acceptance Criteria

- A live session on the private S2S bridge shows `fetch_document` (or the section tool) in the server log, or the fallback above is implemented and shown working instead.
- The spoken-yes latency and the citation check are recorded on this issue.

# Notes

[watermelon-full-preview] mu-docreader, thread 214. Split out of #1040 documents-voice-agent at its close; the deployed-cascade proof and all latency numbers are recorded there.
