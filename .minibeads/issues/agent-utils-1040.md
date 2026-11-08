---
title: 'documents-voice-agent: let the voice agent read and answer questions about the open document'
status: open
priority: 1
issue_type: task
assignee: mu-docreader
created_at: 2026-11-08T01:19:06.000000000+00:00
updated_at: 2026-11-08T01:19:06.000000000+00:00
---

# Description

Owner (thread 214, reply 213): the reader retains the ability to activate the S2S voice agent, but that only makes sense if the agent can actually read the document on screen and answer questions about it. Push a short prefix of the document to the agent, tell it to run a tool for more, and have it open with "Should I read the whole document?" with a quick yes/no. The owner also asks: if I say yes, what is the latency for it to ingest a ~4000-word document?

Today starting the voice agent while a document is open gives the agent no document context and no way to fetch any.

# Design

- When the S2S voice agent is started while a document is open in the reader, the session context gains the document title, the first ~300 words, and the outline, plus a tool to fetch more of that document (by section, or the whole document). The agent's opening move asks "Should I read the whole document?" and a spoken yes/no drives the tool fetch. Questions about the document are answered from fetched text, citing sections.
- The tool reads through the existing documents API (`GET /api/v1/documents/{key}`); a section-fetch parameter is added only if slicing the already-fetched content cannot serve. A documents-view to voice-view hook (current open document key/title/prefix/outline) is the only coupling; no voice.js edit without naming the range to the coordinator first.
- Measure the ingest latency for a ~4000-word document: time from the user's yes to the agent's first spoken words, and the tool round-trip on its own, on the real voice stack where possible and otherwise on a faithful harness with the gap named. Report the numbers on this issue.

# Acceptance Criteria

- Browser check with real input (fake voice backend where needed): starting the agent with a document open delivers the prefix + outline context and the fetch tool; the agent asks whether to read the whole document; yes triggers the tool, a follow-up question is answered from the document with a section cited; no leaves the document unfetched by the agent.
- Starting the agent with no document open is unchanged.
- The ~4000-word ingest latency (yes to first spoken words, and tool round-trip) is measured and recorded on this issue.

# Notes

[watermelon-full-preview] mu-docreader, thread 214 batch C. Sibling work: #1038 documents-outline-popup, #1039 documents-read-parity.

## Resolution (partial — latency, 2026-11-07 night, mu-vdesign)

[watermelon-full-preview] Tool round-trip measured in-process through the real
router (direct JSON-RPC `tools/call`, seeded cached 3,835-word document, debug
build, n=20 after one warm call; throwaway probe kept outside the tree):
`fetch_document` p50 2.08 ms / p90 2.28 ms (result 12,661 chars — the 12 KiB
cap with continuation tail), `fetch_document_section` p50 1.53 ms / p90
1.61 ms (3,414 chars). The tool is not the ingest cost: a whole-document fetch
hands the voice model ~3.2k tokens (one section ~0.85k), so yes-to-first-audio
will be dominated by model prefill + first-token time, not by this server.
End-to-end yes→first-audio is NOT yet measured: nothing in-repo proves the
private S2S bridge attaches `/mcp` and emits tool calls, so that number needs
one live session on the deployed stack (post-deploy). The harness for it
landed with this work: `voice-read-acceptance.py` document mode
(`--document-notice-file` + `--prompt-file` as the answer, `--document-arm`
whole/section/prefix-only; typed T0, spoken-yes recognition not modelled),
reporting notice/answer first-audio median+p90 over ≥5 trials per arm.
