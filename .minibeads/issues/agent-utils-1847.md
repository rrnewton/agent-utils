---
title: 'vibe-talk-slack-communication-audit: repeatable audit that the page never contacts Slack directly and Slack lag never blocks reads'
status: closed
priority: 0
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T10:20:00-05:00
updated_at: 2026-11-18T10:20:00-05:00
---

# Description

Owner direction (thread 183): the client must NEVER contact Slack directly; even send goes through the vibe-talk server; upstream Slack lag lives on the server and must not block anything that does not strictly require it (async fetches). Today's RATE LIMIT warnings are server-side (slack timeline 103, live 9, summarize threads 2, recent 1). Build this as a repeatable audit in validation, not a one-off check. No live Slack calls from tests; fake upstreams only. Dynamic-testing standing rule (thread 185): owner-facing proof goes end-to-end through the real server router with the page's exact requests, stubbing only external upstreams.

# Design

- Bundle gate: a fast, condition-based check in validate fails on any Slack API host/URL (`slack.com/api`, `api.slack.com`, `hooks.slack.com`) in the built page bundle. Workspace permalink hosts (`*.slack.com/archives/...`, `slack://`) are user-initiated navigation links the server hands the page, never fetched by the page; the browser capture below is the check that the page never *contacts* them.
- Browser gate: a browser test captures every network request while loading and using the page against the real server router (fake Slack upstream) and fails on any request whose host is not the page's own server origin.
- Send proof: a test through the real router posts a message and asserts the fake Slack upstream received it via the server and the browser made no non-origin request; a direct client send path would fail the origin capture.
- Lag isolation: with a slow/rate-limited fake Slack upstream, `/timeline`, `/stream`, `/client-config` and Discuss reads are answered cache-first from the server's copy without waiting on Slack, asserted on response conditions (served-from / body), never wall-clock budgets under load. Slack fetches run async/background behind the answer.
- Timeline pacing: the background timeline reader re-reads moved threads only; add pacing/backoff so repeated rate limits widen the interval and a fake-upstream call-count test proves unchanged threads cost no `conversations.replies` calls and total calls stay well under the tier budget.

# Acceptance Criteria

- `make validate` (selected) includes the bundle Slack-API scan and the browser origin capture; both fail when a Slack API URL or off-origin request is introduced.
- Through the real router with a fake Slack upstream: a page send reaches the fake upstream via the server; the browser capture shows only the server origin.
- With the fake upstream delayed/rate-limiting, timeline/stream/client-config/Discuss reads return the cached answer (condition-asserted) and the upstream call count does not grow per page read.
- Fake-upstream call-count test: a second background pass over unchanged threads makes zero new `conversations.replies` calls; after rate limits the pacing interval backs off.
- Screenshot matrix per standing acceptance (light/dark, keyboard open really shrunk/closed, phone 412x915 scale 2.6 and desktop 1280x800, Discuss half-pane open in phone shots) on a local test instance with fixture/fake data, every image inspected.

# Notes

Owner-priority, deployed on landing. Findings note sent to coordb before larger changes.

## Resolution

[watermelon-full-preview] Landed the repeatable audit. Bundle gate: `page_build.test.mjs` fails on any Slack API endpoint (`slack.com/api`, `api.slack.com`, `hooks.slack.com`) in the built page; workspace permalinks are navigation-only by design (liaison-confirmed). Browser gate: new `slack_origin_browser.py` in validate records every request in Chromium against the real server binary and fails on any non-origin or Slack host (read + composer send exercised; 11 requests, all origin). Send proof and lag isolation: real-router tests in `slack_deployment.rs` against the fake Slack upstream — a page send reaches the fake as exactly one server-sent `chat.postMessage`; with the fake rate-limiting, timeline/client-config/Discuss-preview reads answer from the server cache, a send still goes through, and page reads send nothing themselves. Pacing (from the live `slack_calls` tally: refresh dominant at the configured 20/min replies ceiling, limiting at 11 calls/74s — a shared bridge bucket, not the nominal tier): `budget.rs` learns an observed ceiling per method for refresh/background only (halve on each limited answer, floor 4/min, +1 per 10 quiet minutes, INFO-logged; interactive and writes always keep the configured ceiling, unit-tested), Flat refresh fan-out capped at 8 threads/pass, and `auth.test`/`users.info` now tally as `identity` instead of `untagged`. Screenshot matrix on a fake-data test instance: /tmp/mu-slackaudit-shots-1847/ (8 shots, light/dark × phone 412x915/desktop 1280x800 × keyboard open really shrunk to 579/464 and closed, Discuss half-pane open in phone shots), all inspected, no error banner.
