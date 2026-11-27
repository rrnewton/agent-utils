---
title: 'vibe-talk-browser-load-known-red'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-24T06:00:00-05:00
updated_at: 2026-11-26T19:30:00-05:00
---

# Description

Several vibe-talk browser steps fail often under host load (browser closed, waits expiring) while passing on quieter runs: the screenshots family, the Discuss family, and the search/read timing family. Each gets a separate entry in validation/known-red.toml under this tracking issue, with a shared 30-day expiry, so gates whose only reds are those steps can be waived at once while the root-cause work proceeds.

Entries (seeded 2026-11-24, all expiring 2026-12-24): vibe-talk.shots.screenshots-desks, vibe-talk.shots.screenshots-phones, vibe-talk.shots.screenshots-wide, vibe-talk.check.discuss-browser, vibe-talk.check.discuss-dock-browser, vibe-talk.check.discuss-strike-live-browser, vibe-talk.check.discuss-readjump-browser, vibe-talk.check.search-unmatched-browser, vibe-talk.check.read-pause-reply-browser, vibe-talk.check.search-typing-fast-browser.

thread-pin-picker is deliberately NOT listed: its frequent red was a real date bug, fixed in #2158, and reds there are real now.

# Acceptance Criteria

- Every interim entry is a separate known-red.toml entry marked #2175 with the shared expiry; an entry drops off when its step is proven fixed or hardened, and the whole set is renewed only under a live tracking issue.

# Notes

Filed by the coordinator from the browser-red plan (mu-clientperf, 2026-11-24); the owning lane refines this file as it works.

## Triage data, 2026-11-26 (cl-flaketriage)

Runs on main (tree = main ecbdb8f1 plus test-only DAG files, the same page and harness), load1 sampled at each run's start and end; a run counts toward a flake/robust verdict only when it stayed under load1 237 (75% of 316 threads), and runs above that are overload data.

- Desk screenshot walk (`run.sh --screenshots --theme dark --profiles desktop,laptop-1280,desk-1000`): 6 of 6 runs under the bar passed with all 126 desk scenes (load1 170-227), scenes 52-elision-word-count and 82-agent-replies-jump included each time. 3 runs at load1 500-629 failed, each at a different early scene (49-failed-send-and-draft, 01-signed-out, 13-jump-to-newest) with the walk's TimeoutError: overload. A run that started at 500 and fell to 271 passed.
- Relayed by the coordinator: on a later #2258 head the desk step run alone died at scene 55-thread-card-header (Page.click 30 s timeout, browser closed) while load1 rose 345 to 457 (another team's builds); the same step passed twice minutes earlier at load1 170-345.
- The early `TargetClosedError ... Future exception` lines appear in passing desk walks too; the failure signature is the `state ... was NOT reached ... (TimeoutError)` line.
- discuss-browser: 5 of 5 runs under the bar passed (load1 178-193, about 22 s each). Its learned-cap OOM NO_RESULTs (18 in lane gate output) are `#2319`. Its other failures in lane gate output: 4 `wait_for_function` timeouts on heads that do not touch the Discuss page (load) and 2 `close/reopen prepared a different key` assertions, not reproduced on main.
- Over the 7 days to 2026-11-26 the screenshots-desks reds fall on 62 heads that change page code (vibe-talk/web, src or the harness) against 34 that do not, and most of those 34 are wholesale browser deaths (9 or more steps red in one gate).
