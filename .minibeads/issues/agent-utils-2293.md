---
title: 'vibe-talk-read-modes-held-walk-thread-option-timeout: the held walk waits out ten wall-clock seconds for the thread option under load, on unmodified main'
status: open
priority: 1
issue_type: bug
assignee: mu-statesync
labels:
- vibe-talk
created_at: 2026-11-26T10:18:00-05:00
updated_at: 2026-11-26T13:45:00-05:00
---

# Description

`vibe-talk.check.read-modes-browser` times out in `held_walk` at `vibe-talk/tests/read_modes_browser.py:882`: `Page.wait_for_function: Timeout 10000ms exceeded` waiting for the `thread:spaces/A/threads/canary` option of `#thread-select` after pointerdown. Reproduced on unmodified main with no server change involved: the check serves its timeline from an in-process fake API, so no Rust server code runs in it.

Evidence (2026-11-26, host load1 in parentheses, one run at a time, direct `tests/read_modes_browser.py` with the gate's page build): on `afb8ecaa` 5/5 passed (123.64, 126.03, 132.40, 129.83, 130.57); on main `a724562b` 2/3 passed (139.73, 145.53) and the third failed at the same line and wait (149.73). The S3 gate and its exclusive rerun failed the same wait on `afb8ecaa`; S3 landed under a logged waiver for exactly this proof. The wait is a fixed ten wall-clock seconds for a fetch-plus-render to complete — under a hot host that is a wall-clock verdict, which owner rule 346 forbids in gating checks.

# Design

Make the held walk's thread-option wait condition-based end to end, in the pattern of the returned behaviour-only checks: wait on the state that proves the thread list landed (the directory read's own completion the page already tracks), with a generous hang backstop only, never a ten-second wall-clock pass/fail on load. If that cannot be made robust on a hot machine (>75% CPU on 316 hardware threads), disable the step from gating with this issue as its tracker until it can. Either way the proof bar is repeated passes alone and on main at comparable load, with load1 recorded per run.

# Acceptance Criteria

- `read_modes_browser.py` (held walk included) passes repeatedly alone on the fixed tree and on main at comparable host load, with no wall-clock pass/fail wait remaining in the held walk's thread-option path (hang backstops excepted).
- The step returns to (or stays in) gating only on that proof; if disabled instead, the disable names this issue and the step's solo runs are recorded here.

# Notes

Filed from the #2280 S3 gate on `afb8ecaa` (ticket 1795702247402631538, read-modes the only functional red; landed under waiver as `dce61a5b`). Related: `#2077 vibe-talk-browser-checks-hot-host`, `#1250 vibe-talk-timing-robust` (the behaviour-only return pattern), `#2280 vibe-talk-state-sync-audit` (the gate that surfaced it). No server change is wanted for this issue; the check's fake API is the whole of its read path.

Update 2026-11-26 11:48 AM ET: a longer backstop is not the fix. In a temporary experiment (reverted before the disable commit), the same held-walk wait with a 120-second backstop passed 4 direct runs and then wedged for the full 120 seconds with the thread option still absent. The condition can therefore fail to arrive at all on some runs, not merely arrive slowly: this points at a real page or fake-API bug (the option never appears) that a condition-based rewrite must root-cause, not paper over. The step was disabled from gating instead of keeping the backstop change: 05418230, landed gated as d7ce9c3b (full gate 215/217 ok, the two reds being unrelated CPU timeouts that passed on exclusive rerun). This issue stays open for the real fix.

Resolution in progress (2026-11-26): the wedge is root-caused and it is not the timeout. Under host load the page takes a longtask and its freeze-watch diagnostics POST to /api/v1/freeze-report; the check fake's do_POST answered 404 without reading the body, so on the HTTP/1.1 keep-alive connection the JSON body was parsed as the next request line (stdlib 400/501) and a pooled fetch - the picker touch's view=threads read - could draw that error as its whole answer. refreshThreadDirectory then recorded the failure quietly with nothing retrying while the picker stood open, so the thread option never arrived (the 120 s observation) until a re-touch. Fix, three layers: the fake consumes POST bodies (the sweep is #2307), the page retries the directory fetch leg once inside the same single-flight (a persistent failure still surfaces; proven with an always-failing fake: exactly two fetches, then the failure), and held_walk's waits name their page condition with HELD_BACKSTOP_MS as a hang backstop only. The step returns to gating in the same commit.
