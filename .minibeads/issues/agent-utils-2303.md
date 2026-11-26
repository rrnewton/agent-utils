---
title: 'vibe-talk-archive-single-row-desk-windowed-flake: the desk-windowed leg fails about half its runs on unmodified main, the acted row never showing archived'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-26T11:52:00-05:00
updated_at: 2026-11-26T11:52:00-05:00
---

# Description

Found on the `#2283` C1 gate (400e79e4, 2026-11-26 11:40 AM ET), whose only red step was `vibe-talk.check.archive-single-row-browser` (229 passed, 1 failed; gate log `~/.local/state/agent-utils/gate-logs/1795708839994747007-011b4246/`). Run alone 6 times a side the same day (mu-hasrules; `VIBE_TALK_WEB_ROOT=target/web python3 tests/archive_single_row_browser.py`, nice, host load1 recorded per run, 135-147 — not hot by the rule):

| Tree | Runs | Pass | Fail |
|---|---:|---:|---:|
| b81f7674 (unmodified main, C1's base) | 6 | 3 (runs 1, 3, 6) | 3 (runs 2, 4, 5) |
| d74edc93 (`#2283` C1 rebased; patch-id unchanged) | 6 | 2 (runs 1, 5) | 4 (runs 2, 3, 4, 6) |

Every failure is the identical first assertion of the desk-windowed leg (1280x800, windowed lists on): `AssertionError: desk-windowed: the acted row did not show archived=True` (`press_e_and_check`, tests/archive_single_row_browser.py:99, after its 25 s condition wait). The phone leg (412x915, windowing off) passed 12 of 12. Logs: `/tmp/mu-arch-{mine,mine2,parent,parent2}.log` on devbig014 (mu-hasrules lane).

The sequence under test: the walk selects the newest row with a script `row.scrollIntoView(); row.click()`, waits a fixed 300 ms, presses `e`, then waits for that row's `data-archived` to read `true`. The `e` chord acts through `doneFromKeyboard` (`view/shell/donekeys.ts`) on `rowForAct`'s selection; a keyboard act on an unmounted selection is a deliberate no-op (the check's own later leg relies on exactly that).

# Design

Read as a test race over a real windowed-selection edge, not an archive bug, on this evidence:

- The archive write path is untouched by the trees compared and the non-windowed leg never fails: the store write, the dress (`data-archived` from `rowdress.ts`) and the `e` chord all work whenever the acted row stays mounted and selected.
- With windowing on, `scrollIntoView()` on the newest row starts the windowed renderer's deferred re-slice (the `requestAnimationFrame` re-slice behind a queued flag that `#2292`'s (c)2 names). Whether the selected row is still the mounted, selected row when `e` lands 300 ms later is a race: when the re-slice has unmounted or replaced it, `e` takes the designed unmounted-selection no-op and nothing is ever archived, so the 25 s wait cannot recover. Which side of the race a run lands on is timing, hence about half the runs at the day's load on either tree.
- The owner-facing edge worth confirming (for `#2292`, mu-windowfoot): a real keyboard reader whose selected row is unmounted by a re-slice between selection and keypress gets the same silent no-op. Windowing defaults OFF today (`#2292` gates turning it on), so no owner hits this yet; selection surviving a re-slice (held by message id, re-dressed on mount) belongs on `#2292`'s exercise list beside search, jump-to-message and Back.

Fix the check, not the page, first:

1. Select the row the reader's way (a real pointer click through Playwright's locator, which scrolls and focuses for real) or wait on conditions instead of the fixed 300 ms: the acted row mounted, selected, and the window settled (mounted count stable across two frames), before pressing `e`. Condition-based only (rule 346, no new wall-clock budgets; the existing waits are hang backstops).
2. Keep every bound and assertion as they are (dressed rows, layout reads, class writes; the unmounted-selection no-op leg).
3. If the leg still fails with the selection provably mounted at the press, that is a page bug: reopen against `donekeys`/`renderWindowed` with the failing run's trace, and cross-link `#2292`.

# Acceptance Criteria

- `tests/archive_single_row_browser.py` passes 10 runs in a row on unmodified main at 412x915 and 1280x800, including at host load1 at or above 237, with load1 recorded per run.
- The desk-windowed leg still proves its bounds (dressed rows, layout reads, class writes) and the unmounted-selection no-op, unchanged.
- The selection-across-re-slice question is answered one way or the other in Notes (selection by id survives a re-slice and the remounted row dresses from the store, or a follow-up is filed under `#2292`).

# Notes

Related: `#1990 vibe-talk-archive-single-row-dress` (the check and the single-row dress it guards), `#2292 vibe-talk-windowed-composer-foot-fight` (the windowing-on hardening epic; its deferred re-slice is the other party in the race, and its Notes' stacking puts `#2283` before it), `#2283 vibe-talk-root-has-rules` (the gate that surfaced this; its C1 was cleared of the red by the 6+6 runs above and the step is being handled under the flake rule there). Filed by mu-hasrules at coordb's direction, 2026-11-26.
