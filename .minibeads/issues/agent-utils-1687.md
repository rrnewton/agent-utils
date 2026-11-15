---
title: 'timeline-browser-fixed-port: the timeline browser fixture binds a derived port that a busy host program can break'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-15T21:40:00+00:00
updated_at: 2026-11-15T21:40:00+00:00
---

# Description

## Goal
`timeline.browser.functional` must not fail before a single test runs because an unrelated program on the host already listens on the port its Playwright webServer wants to bind. Pick a free port at run time, pass it to the webServer and the baseURL, and pin the behaviour with a test proving a busy port cannot break it.

## Evidence
During the #1591 gate the step failed in 1s with `Error: listen EADDRINUSE: address already in use 127.0.0.1:36781` and `Process from config.webServer was not able to start`. The config derived port 36781 from this checkout's path (`py/tests/js/browser/playwright.config.cjs`), and an unrelated long-running host daemon held that port (`ss -ltnp` showed a listener on `*:36781` owned by a process this change does not touch). The same suite passed 39/39 solo with `AGENT_TIMELINE_E2E_PORT` pinned to a free port, proving the failure is the bind, not the tests. A solo gate `--rerun` of the step failed identically while the port stayed held. Every gate selecting this check fails the same way until the port is chosen at run time.

## Fix
Extract port selection into `py/tests/js/browser/port-picker.cjs`: a pinned `AGENT_TIMELINE_E2E_PORT` is honoured as given (a bad pin is refused, a busy pin fails loudly); otherwise the checkout-derived port is probed and used when free, and an ephemeral port the kernel picks is used when it is busy or unbindable; no free port at all is an error naming the derived port. `playwright.config.cjs` passes the picked port to the webServer command and the baseURL and writes it back to `AGENT_TIMELINE_E2E_PORT` so spawned processes loading the config agree. Add `py/tests/js/browser/port-picker.test.cjs` (decision table with injected probes, plus a real-socket busy-port case) and a `timeline.browser.port-picker` step in `validation/timeline.dag.yaml`.

## Acceptance
With the derived port held by another socket, the timeline browser suite starts its fixture server on a different free port and runs; `port-picker.test.cjs` passes including the real busy-port case; the pinned-port behaviour is unchanged; the validation graph contract (each `*.test.cjs` owned exactly once) still passes.
