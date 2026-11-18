---
title: 'vibe-talk-route-health-alert: alert when an /api route fails 100% of a window'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- ops
created_at: 2026-11-18T10:30:00-05:00
updated_at: 2026-11-18T10:30:00-05:00
---

# Description

[watermelon-full-preview] From #1844 vibe-talk-discuss-followup-422 (owner thread 185): POST /api/v1/channels/*/catchup/messages returned 422 for 19 of 19 requests across 5 days and nobody noticed until the owner hit it. A route that refuses every request is indistinguishable from a working one in per-request logs. Any /api route where 100% of requests over a window are 4xx/5xx should raise an alert to the coordinator.

# Design

- In-process counters in the access layer (it already sees every request, method, path and status): key = method + route template (id-shaped path segments folded to {id}), value = per-window total / 4xx / 5xx counts plus last status. No new log scraping and no per-request writes.
- Evaluation on the existing ops tick: a route alerts when its window has at least 5 requests and zero successes (100% 4xx/5xx). One alert per route per window, naming route, count, window and dominant status/code; the alert goes to the coordinator, not the owner.
- Exposure for diagnosis: the counters ride an existing health/ops surface (health endpoint detail or an authenticated ops read), so an alert can be confirmed without reading raw logs.
- Windows and thresholds are constants first; configuration only if a second consumer needs it.

# Acceptance Criteria

- A test drives a route to 100% 4xx over the minimum sample and shows exactly one alert for that route and window; a route with any success, or below the sample floor, alerts never.
- A test shows id-shaped path segments fold into one route key (two channels count together).
- The health/ops surface shows per-route totals and failure counts matching driven traffic.
- No change to request handling or the access log line; counters cost one map update per request.

# Notes

Follow-up to #1844, filed at the coordinator's request (owner thread 185). Design sent to the coordinator before building; this issue does not block the #1844 fix.

## Resolution

[watermelon-full-preview] Landed on main as 1066b089 (gate pass; combined tree verified against the api.rs/integration.rs overlap, 48 focused tests there). The access layer counts every /api/ answer per folded route into a one-hour in-process registry; a minute tick closes each window and raises an ERROR line for any route with at least five requests and no success; /healthz carries the top-20 route counts and the last window's alerts.
