---
title: 'vibe-talk-browser-checks-hot-host: browser checks time out en masse in gates on a busy host'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- testing
created_at: 2026-11-21T14:50:00-05:00
updated_at: 2026-11-21T14:50:00-05:00
---

# Description

On 2026-11-21, with the host at about 70% CPU and three gates running, full and path-selected gates of unrelated changes failed 4 to 14 vibe-talk browser checks each (Playwright click or wait timeouts, no assertion failures), on different checks each run; each passes when rerun alone. The owner's rule is that every gating check must pass on a host above 75% CPU and nothing timing-sensitive gates.

# Design

Find why: concurrent Chromium instances inside one gate's CPU allowance, page builds competing with browser steps, or fixed 30 s waits. Then make the browser checks robust on a loaded host: bound browser-step concurrency inside a gate (a dagrun resource), wait on conditions instead of fixed timeouts, and share one built page per gate. Quarantine any check that still fails under load, with a pointer to this issue.

# Acceptance Criteria

- The vibe-talk browser checks pass in a gate with the host above 75% CPU, repeatedly.
- No browser check relies on a wall-clock budget for its verdict.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

## Diagnosis (2026-11-21, mu-browserload)

[watermelon-full-preview] Three stacked bounds starve Chromium, then fixed wall-clock waits convert starvation into rotating timeouts:

- Scheduling: 158 runnable browser steps hold `browser: 1` under cap `browser: 8`, per gate process only (no shared resource-caps path), so three concurrent gates run up to 24 Chromiums (27 counting `check.keyboard-browser`, whose fragment declared no `browser` resource at all). Inside a gate's 48-CPU allowance (#1254), each browser step was boxed to one core (no `preferred_inner_jobs`, dagrun's small default) beside CPU-bound builders admitted on the same ready set; admission never sums CPU demand.
- Page build: 134 fake-server walks each ran `node web/build.mjs` per Python process (`page_sources.served` caches per process only), about 134 esbuild builds per gate bursting beside sibling Chromiums. The 30 real-server checks already share `build.release`.
- Waits: 1190 clicks, 220 gotos and about 871 locator `wait_for`s carry no timeout and inherit Playwright's 30 s default; only two files set a hang backstop. Explicit short bounds (3000 ms welcome gate in 104 files, 10000 ms in 93) are likewise wall-clock verdicts. No shared waiting helper exists; the `STATE_S` poll-then-`check` idiom and #2012's condition wait are copy-pasted.

Gate ledger rows a4100efa, e1ecd02f, bf300e59, 3d872668, d2d5db8f share no single failing step; `offline-cache-phone-360` fails in four of five. Step output is not retained anywhere durable (ledger `log_path` is always null; no `DAGRUN_LOG_DIR` is set), tracked separately by the coordinator.

## Fix in progress

- A (scheduling + build): cap `browser: 8` to `4` in both fragments (sole-holder demands follow), `preferred_inner_jobs: 4` on every browser step (browser lane ceiling 16 of the gate's 48 CPUs), the missing hint on `keyboard-browser`, and one `build.page` step per gate serving all fake-server walks through `VIBE_TALK_WEB_ROOT` (134 page builds to 1; a stale build refuses by recorded input digests).
- B (waits): shared `browser_waits` helper (hang backstop defaults + condition waits) applied suite-wide.
- C (proof): the browser group repeatedly above 75% host CPU; quarantine only what still fails, as `disabled-optin` with a pointer here.

## Progress

[watermelon-full-preview] A landed on main as a3caaea0 (gate e91e0755 red only in the #2065 families and 4 load-timing browser checks, coordinator waiver). The exact route-shape classification for the offline-cache fixture's channel reads is its own commit on this branch.

B adds `vibe-talk/tests/browser_waits.py` (`HANG_BACKSTOP_MS`, `hot_host_timeouts`, `wait_until`, `click_when_clear`) and applies the backstop at all 304 context/page creations in 160 files (AST-guided; mypy strict clean). Hand conversions at Gate A's four failure spots: the kinds walk reaches the call view by keyboard (at 150% root the Documents view lays out 518px wide on the 360px screen, putting `#view-switch` past the visual viewport once that frame commits — a coordinate click only lands by racing the frame; the placement assertions are unchanged), the done-marking animation is polled for instead of sampled once, the one-page picker read waits on its condition, and the shared `Walk.until` bound becomes the hang backstop. New walks should use `browser_waits` (one sibling lane's in-flight walk adopts it after this lands).

Follow-up worth its own look (not fixed here): the Documents view overflowing to a 518px layout at 150% root type on a 360px phone, and a host-wide browser cap across concurrent gates (the per-gate cap is process-local; three gates can still run 12 Chromiums — halved from 24 by A, and each now has a four-core box).
