---
title: 'vibe-talk-document-load-502-readable: a document load that fails with a 5xx or non-JSON reply says what failed and offers Retry'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-25T10:15:00-05:00
updated_at: 2026-11-25T12:15:00-05:00
closed_at: 2026-11-25T12:15:00-05:00
---

# Description

Owner report (2026-11-25, open-source deployment, possibly not current): clicking a link to a document opened the Documents reader, which failed to load it and showed only "vibe-talk returned non-JSON (HTTP 502)".

Two halves:

- Page: the API client turns any reply whose body is not JSON into that bare sentence, with no status or detail kept, and the reader's failure text (`#1980`) falls back to it verbatim. There is no way to try again from the reader.
- Server: the server's own document fetch failures are JSON (`document_fetch_failed`), so a non-JSON 502 is the page of whatever fronts the deployment (a tunnel or reverse proxy) when the origin never answers. The GitHub fetcher's HTTP client has no connect or overall timeout, so a stalled upstream or egress connection holds `POST /api/v1/documents/fetch` open until the front end gives up and answers with its own error page. A handler panic would end the same way (the connection drops and the front end answers 502).

Related: `#1591` (open-source Documents view empty; its container build fix is landed) covers links that did not open at all, not this failure path; `#1980` gave server failures a specific reason; `#1844` made every `/api/` 4xx JSON.

# Design

- Page: the API client keeps the HTTP status on a non-JSON reply and marks it as not from vibe-talk. The reader's failure says in plain words what failed (the document could not be loaded because the server or what fronts it answered with an error page or did not answer), names the document's host and the HTTP status, and offers a Retry button that runs the same load again, beside the existing "Open on GitHub" link. The raw "vibe-talk returned non-JSON" string is never shown.
- Server: every document fetch runs under one deadline shorter than common front-end timeouts, and the GitHub client has a connect timeout, so a stalled upstream becomes a JSON error naming the host and the cause instead of a front end's bare 502. A panic in an `/api/` handler answers a JSON 500 with a cause instead of dropping the connection.
- Tests: a page test for a non-JSON 502 and a 5xx JSON reply in the reader, including Retry; a real-router server test with a stalled stub upstream. Condition-based waits only.

# Acceptance Criteria

- A document load answered by a non-JSON 502 (an HTML error page) shows a readable sentence naming what failed, the document's host and the status, and a Retry button; the bare "vibe-talk returned non-JSON (HTTP 502)" is never shown. Retry re-runs the load and a load that then succeeds shows the document.
- A 5xx JSON failure keeps the server's specific reason and also offers Retry.
- Through the real router, a fetch whose upstream never answers returns a JSON error (`document_fetch_timeout`) naming the host within the server's deadline, never a hang or a bare 502.
- A panicking `/api/` handler answers JSON 500 with a cause.
- Screenshots of the failure state at phone 412x915 and desktop 1280x800, light and dark, on a local test server.

# Notes

Reproduce and test only on a local server from the worktree build, with stub upstreams and a local front end; never the live instance.

## Resolution

Landed on main as ba287b68 (replay of 32d21bee, which passed the owner-tier gate: 225 steps, no failures); deployed 11:57 AM ET on 2026-11-25, deploy watch clean to 12:12 PM ET.

Root cause: the GitHub fetcher's HTTP client had no connect or overall timeout, so a stalled upstream held `POST /api/v1/documents/fetch` open until the deployment's front end gave up and answered its own HTML 502 page; the page's client turned that body into "vibe-talk returned non-JSON (HTTP 502)" and the reader showed it verbatim, with the same text as a status toast and no way to try again.

Fix: every document fetch and freshness check runs under a 45 s deadline and the GitHub client has a 10 s connect timeout, so a stalled upstream answers JSON 504 `document_fetch_timeout` naming the upstream. A handler panic answers JSON 500 `internal_error` with a fixed sentence and logs exactly one ERROR line (method, path, cause, panic message). The reader's failure says whose answer it was (vibe-talk's cause, or the proxy or tunnel in front of it with the document's host and the status), offers Retry (disabled "Retrying…" while it runs), and is no longer repeated as a status toast; the list card uses the same wording.

Evidence: real-router tests for the stalled upstream (injected deadline, stub that never answers) and for the panic (JSON 500 plus one ERROR line); page tests drive the real API client over a proxy's HTML 502, a vibe-talk 504, a gateway's JSON 5xx and a Retry that lands with nothing left on the status line. A local reproduction behind a stand-in proxy showed the owner's exact string on the old build and the readable failure, Retry and vibe-talk's own 504 on the new one, at phone and desktop sizes in light and dark. The wall-clock test found along the way is #2245.
