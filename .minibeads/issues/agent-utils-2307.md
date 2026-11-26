---
title: 'test-fakes-unread-post-body-keepalive-poison: a fake that answers a POST without reading its body poisons the keep-alive connection for the next fetch'
status: open
priority: 1
issue_type: bug
assignee: mu-statesync
labels:
- vibe-talk
- tests
created_at: 2026-11-26T13:32:00-05:00
updated_at: 2026-11-26T13:32:00-05:00
---

# Description

Browser-check fakes built on `BaseHTTPRequestHandler` with `protocol_version = "HTTP/1.1"` keep each connection alive for the next request. A handler that answers a POST without reading its `Content-Length` body leaves those body bytes in the socket; the handler then parses them as the next request line and answers the stdlib's own 400/501 (`Bad request syntax` / `Unsupported method ('{"v":1,...}')`) and closes. The browser pools that socket, so a later fetch assigned to it can receive the parse error as its whole answer (a non-JSON 400/501) even though its own request was well formed and is never seen by `do_GET`.

Proven in `#2293` (2026-11-26): under host load the page takes a longtask, its freeze-watch diagnostics POST to `/api/v1/freeze-report`, the read-modes fake's one-line `do_POST` 404s without reading the body, and the picker touch's `view=threads` GET drew the poisoned socket and failed. Because the trigger is a jank-driven telemetry POST racing a pooled fetch, the failure looks like a load flake in whatever check happens to be touching the page, and no wall-clock budget distinguishes it from slowness.

Textual sweep of `vibe-talk/tests/*.py` (2026-11-26; criterion: declares HTTP/1.1 and its `do_POST` never reads the body before answering, an early return before the read counting as not reading):

- Never reads the body: `read_modes_browser.py` (fixed under `#2293`), `thread_split_regroup_browser.py` (identical one-line 404).
- Reads the body only after an early return: `bookmark_browser.py` (`authorized()` answers 401 and `do_POST` returns before `body()` runs; today's beacon is authorized, so this file is latent rather than live).
- The other 30 HTTP/1.1 fakes read the body in `do_POST`; the 10 with `do_POST` but no HTTP/1.1 declaration close per response and are not exposed. Treat the sweep as a starting list, not ground truth: re-verify each file's `do_POST` (helper indirection included) before changing it.

# Design

For each affected fake, consume the request body before answering any POST: `self.rfile.read(int(self.headers.get("Content-Length") or 0))` at the top of `do_POST` (or in the helper every path runs), including error and unauthorized paths. Where a fake's `do_POST` exists only to 404, that one line is the whole fix. Do not switch fakes to HTTP/1.0 or `Connection: close` per response instead: connection reuse is part of what the checks exercise, and closing hides the next framing bug rather than fixing this one. If a shared handler helper ever emerges, the read belongs there; until then the per-file line matches how the 30 healthy fakes already do it.

# Acceptance Criteria

- In `thread_split_regroup_browser.py` and `bookmark_browser.py`, every `do_POST` path (success, 404, 401) consumes the declared body before responding, and each file's check still passes solo.
- The sweep is re-run after the change and records zero HTTP/1.1 fakes whose `do_POST` can answer with body bytes unread; any file the textual scan misjudged is named here with what it actually does.
- No wall-clock budget is added or relied on anywhere (owner rule 346).

# Notes

Split out of `#2293 vibe-talk-read-modes-held-walk-thread-option-timeout`, whose fix covers `read_modes_browser.py` itself plus the page-side retry that makes any one poisoned fetch non-fatal to the picker. Related: `#2077 vibe-talk-browser-checks-hot-host` (the broader hot-host flakiness family this hid inside), `#1250 vibe-talk-timing-robust`.
