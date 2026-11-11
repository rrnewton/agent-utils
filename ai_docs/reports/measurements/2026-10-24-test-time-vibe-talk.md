# test-time-vibe-talk: vibe-talk's test suites, measured and cut

[opus 5.5] Findings of the 2026-10-24 test-suite optimization round, vibe-talk half
(`vibe-talk/tests.dag.yaml`). The other half (everything outside vibe-talk) is a separate lane.
The owner's brief: anything over ten minutes deserves inspection, and pruning where its signal is
redundant or out of proportion to its time.

All numbers are from the validation host (316 cores, shared with other agents' gates; the gate
runs under a 32-core per-run CPU cap). "Before" for the whole graph is the most recent full-graph
gate before this work (790.6 s, logged by the `#474 browser-lane-critical-path` lane);
before/after for single suites are runs made here, back to back, on the same host.

## 1. Measured: what was over ten minutes, and what was close

Full-graph gate before this work: **790.6 s wall, 10 853 s CPU**; vibe-talk's 83 steps
**3 381 s wall summed, 3 186 s CPU**.

No vibe-talk step was over ten minutes of wall; one was over ten minutes of CPU, and two were
within ninety seconds of ten minutes of wall:

| step | wall s | CPU s | what it is |
|---|---:|---:|---|
| `build.tests` | 135 | **1 316** | `cargo test --release --no-run`: 89 integration-test binaries |
| `test.page` | 531 | 540 | one node process over 2 291 page tests, after `build.tests` |
| `shots.screenshots` | 536 | 362 | 222 captures of the app, one browser slot throughout |
| `build.release` | 48 | 226 | |
| `check.column-overscroll-browser` | 204 | 24 | |
| `check.approval-card-small-screens-browser` | 179 | 33 | |
| `check.approval-card-desk-browser` | 166 | 25 | |
| `check.approval-card-phone-place-browser` | 156 | 22 | |
| `check.offline-cache-browser` | 152 | 89 | |
| `check.approval-card-phone-alone-browser` | 114 | 9 | |
| `check.scroll-stability-browser` | 110 | 12 | |
| `check.approval-card-phone-help-browser` | 96 | 8 | |
| `check.keyboard-browser` | 91 | 33 | |

The vibe-talk critical path was `build.release` (48 s) → `build.tests` (135 s) →
`test.page` (531 s), about 714 s, beside the browser lane.

**The browser lane is the other bound.** 69 vibe-talk browser steps and 3 timeline ones share
`resource_caps: browser: 4`. Their walls sum to 2 648 s (1 025 s CPU — they are waiting, not
computing), so with four slots the lane cannot finish in less than 662 s.
`shots.screenshots` alone was a fifth of it.

### The slowest cases

The only per-case records vibe-talk had were libtest's, where the whole page suite was one case
(530 s). Run with node's own TAP durations (one process, `page-base` below), the page suite's
slowest twenty, before and after the fixture changes in section 2:

| before s | after s | test |
|---:|---:|---|
| 36.3 | 15.1 | `#240 reading-bookmark`: a Done through here whose second batch fails keeps the first Done … |
| 33.3 | 14.4 | `#240 reading-bookmark`: an Undo whose second batch fails leaves what was put back to read … |
| 9.2 | 1.3 | NO KEY MARKS EVERY MESSAGE ABOVE ONE READ ANY MORE … (#295) |
| 7.4 | 2.7 | `#269 approval-card-inline` x `#280 scroll-stability`: BACK FROM HELP OPENED BY ITS KEY … |
| 7.3 | 4.1 | `#240 reading-bookmark`: a long Done through here is ONE request … |
| 5.4 | 1.6 | `#344 search-shortcut`: Ctrl+/ AND Cmd+/ OPEN THE SEARCH FROM ANYWHERE … |
| 3.5 | 1.4 | `#269` x `#280`: A READER WHO SEES ONLY THE SEND CARD … |
| 2.8 | 1.2 | `#269` x `#280`: A READER WHOSE SCREEN SHOWS ONLY THE SEND CARD … |
| 2.8 | 0.9 | `#306 done-stale-across-views`: a Done or an Undo made in the left column … |
| 2.6 | 1.2 | `#269` x `#281 view-keys` x `#280`: LEAVING THE THREAD LIST … |
| 2.6 | 1.3 | `#304 back-gesture-in-app`: A RELOAD in a thread opened from the thread list … |
| 2.5 | 1.0 | `#269` x `#280`: BACK FROM SETTINGS, a reader of a long draft … |
| 2.4 | 0.0 | with threads: a message arriving live is followed by a reader on the newest line … |
| 2.3 | 1.0 | `#269` x `#280`: A READER WHO SAW ONLY THE SEND CARD … |
| 2.2 | 1.0 | `#280` x `#269`: THE BOX TAKING BACK ITS ROOM … |
| 2.2 | 0.3 | `#332 settings-voice-agent-section`: every state line under the three … |
| 2.1 | 0.4 | `#403 composer-key-global`: with a thread open on the right … |
| 2.1 | 1.0 | `#280` x `#269` x `#281`: A READER UP THE THREAD … |
| 2.0 | 0.7 | `#469 replication-model-tests`: the page agrees with the service under random interleavings … |
| 1.9 | 0.3 | `#264 voice-send-approval`: every answer an agent would take for a yes is answered aloud … |

The two `#240` tests build a 1 100-message channel (the page's Done batch is 1 000, so a second
batch needs that many), and the fixture's layout model measures every row by walking every row
above it: quadratic, and most of what is left of them. Memoizing the layout model needs every
mutation path of the fake DOM to invalidate it; a missed one would make the suite pass on stale
geometry, so it is left alone (section 4).

## 2. Changed

### The page suite: `test.page` → `test.page-0..3`

*Profile first.* A V8 CPU profile of one slice put **52 %** of the suite in `markupPlace`, which
split the 269 KB `web/voice.html` into lines afresh for every `FIXTURE_TREE` pair on every
`newPage`; another 9 % was `hasClass` splitting class lists, 5 % compiling the 2 MB `voice.js` per
page, and 16 % idle in `settle()`'s `setTimeout(…, 0)`, a real millisecond thousands of times over.

* `markupPlace` and where each element closes are worked out once (the markup never changes in a
  run); class lists are split once per distinct list; `voice.js` and `contract.js` are compiled once
  as `vm.Script`s and run in each page's context (a negative control's mutated copy is still
  compiled on its own); `settle()` waits one turn of the event loop with `setImmediate`. Everything
  the page awaits in this fixture is a promise (its timers are the test's), so the turn is enough.
* `VOICE_PAGE_SHARD=i/n` runs every n-th test from i. Interleaved, so the slow families, which sit
  together in the file, spread over every slice. A malformed value throws; a slice that selects
  nothing, or a file that defines fewer than the floor (moved here from `tests/voice_page.rs`), fails
  at exit.
* The graph runs four slices as four steps straight through node, with node TAP per-case records,
  **depending on nothing**: the cargo harness only started node and read its counts, and its counts
  now live in the file. So the suite runs while the builds do, not after them. `test.unit` still
  skips the harness by exact name; `cargo test` alone runs it, now as concurrent slices
  (`VOICE_PAGE_JOBS`, default one per core up to eight), checking that the slices between them
  selected every test once.

| | wall s | CPU s | peak |
|---|---:|---:|---:|
| one node, before (`page-base`) | 605 | 622 | 3.7 GiB |
| one node, after (`page-opt1`) | 168 | 209 | 4.1 GiB |
| cargo harness, 4 slices at once | 48 | 208 | 1.2 GiB (largest process) |
| gate, 4 steps (`gate1`) | 32 / 41 / 48 / 51 | 41 / 53 / 58 / 61 | 0.8–1.4 GiB |

Each slice's node is held to `--max-old-space-size=1024`. What a slice keeps alive is a few hundred
MiB, but on a host with a lot of memory V8 sizes its heap to the host and lets garbage reach 2.7 GiB
a slice (found by the review below); held, slice 1 peaked at 0.86 GiB and ran in 46 s against 55 s.

All 2 291 tests pass before and after, with identical names (the set compared from the two TAP
streams); no test name repeats, which the per-case verifier requires.

### The screenshot walk

The walk took 508 s here for 222 captures. **221 s** of that was the walk standing still while each
PNG was decoded and judged in pure Python (about a second per capture; the harness deliberately
uses only the standard library).

* The decoder does the common rows a row at a time: `Up` (two thirds of Chromium's rows) as one
  wide-integer addition that keeps each byte's carry to itself, `Sub` as a running sum per channel,
  `Average` and `Paeth` without a function call per byte, and the pixels sliced out of a row. The
  judge counts colours a row at a time instead of holding every pixel as a tuple (56 MiB peak).
  All 222 captures judge to the same verdict, field for field: 221 s → 103 s of CPU.
* The walk does not wait for the judge: `start_capture` writes the frame and hands it to two spawned
  processes, and each profile collects every verdict, in order, before it is done. A blank frame
  still fails the run by name and exit code; one taken before whatever then stops the walk is
  reported first.
* Its `--self-test` (58 controls) was in no step and was broken by a test double the driver had
  outgrown. Fixed, with a new control for a blank frame judged in another process, and run as
  `check.screenshots-self-test` (2 s) and by `make validate`.

| | wall s | CPU s |
|---|---:|---:|
| walk before (`shots-base`) | 508 | (not recorded) |
| walk after (`shots-opt`) | 311 | 235 |
| gate before (full graph, 2026-10-23) | 536 | 362 |
| gate after (`gate1`) | 280 | 252 |

Per profile, before → after: iPhone 15 149 → 66 s, iPhone SE 66 → 46 s, desktop 81 → 50 s,
laptop-1280 128 → 93 s, Android 412 50 → 35 s, desk-1920 31 → 19 s.

### Review

An adversarial review (one sub-agent, given the commits and not these conclusions) found no
signal lost. It checked the `Up` addition exhaustively over all byte pairs, ran a randomized
differential of the old and new decoder and judge over 4 000 synthetic PNGs of every colour type and
filter (identical results, error messages included), confirmed the page file defines its tests in a
deterministic order with no module-level state a test leaves behind, and that the TAP records match
the per-case verifier. Its findings, all fixed before landing: the slices' memory (above), a crash
other than an unreachable state skipping the drain of earlier verdicts, the passed frames' lines
not printed on that path, and the judging pool created outside its `finally`.

## 3. Whole gate, before and after

| run | steps | wall s | CPU s | vibe-talk steps: wall summed / CPU | vibe-talk done at |
|---|---:|---:|---:|---:|---:|
| full graph before (2026-10-23, `#474` lane's gate) | 174 | 790.6 | 10 853 | 3 381 / 3 186 | ≥ 714 (its build → page chain) |
| full graph after (`validate.py --all --keep-going`, landed tree, vibe-talk rebuilt) | 178 | 862.5 | 9 718 | 2 930 / 2 824 | 645 |
| landing gate (selected: vibe-talk, Python, repository infrastructure; builds warm) | 108 | 593.6 | 1 373 | 2 605 / 1 190 | 594 |

* **vibe-talk is no longer the long pole.** Before, its build → `test.page` chain alone was about
  714 s of the 790 s graph. After, no vibe-talk step is among the eight longest of the full graph, and
  vibe-talk finished at 645 s of 862 s; the rest was `cross.agentctl.differential` (665 s) and the
  Python lifecycle and wrkslots shards, all outside this lane, all slower than the day before on a host
  with a load average in the 30s to 40s.
* **CPU:** vibe-talk's steps went from 3 186 s to 2 824 s (page suite 540 → 221 s, screenshots
  362 → 256 s; `build.tests` unchanged at about 1 330 s, see section 4).
* **What bounds vibe-talk now is the browser lane.** In the selected gates the vibe-talk graph ended
  at 593 s and 594 s, which is the browser steps' summed wall (2 369 s) over the four slots: the
  lane, not any single step, is the critical path (section 4, items 1 to 3).
* The full-graph "after" run is not green, and not because of this work: `rust.dagrun.test`
  (`hung_test_is_terminated_named_and_logged_while_a_bystander_survives`) and `rust.agentctl.test`
  failed under load, outside vibe-talk, as did `cross.agentctl.herdr-agent-differential` in a first
  attempt; and `vibe-talk.check.rich-headers-browser` timed out once clicking a Settings fold at the
  desk. That check passed in both selected gates and in four standalone runs side by side; filed as
  `#493 rich-headers-settings-fold-flake`.

## 4. Not changed: for the owner

None of these are clear-cut; each needs a decision that is not a test-runtime one.

1. **Raise `resource_caps: browser` from 4.** The lane is latency-bound (1 025 s CPU over 2 648 s of
   wall) and is now the vibe-talk bound. The cap is stated in two fragments
   (`vibe-talk/tests.dag.yaml`, `validation/timeline.dag.yaml`), which must agree, and it was set
   with laptops in mind. A host setting (validate.env) that lifts it where cores allow would keep
   the laptop default.
2. **Fixed waits in the browser checks.** Measured by wrapping `page.wait_for_timeout`: the share
   of each check's wall spent in fixed waits — approval-card phone-help 92 % (90 s of 97),
   scroll-stability 89 % (99 of 111), column-overscroll 88 % (179 of 203), keyboard 64 %
   (63 of 99; 58 s is one 40 ms `settled()` pause called 1 438 times), thread-card-header 51 %
   (one 25 ms poll 1 500 times), offline-cache 31 %. Some are the property under test ("nothing
   moves for 2.5 s"); many are paddings ("let every read land", 300–400 ms) that a condition (the
   fake API's requests in flight, then frames) could replace. Converting them is per-wait work with
   flake risk on a loaded host, so it wants the owner's go-ahead and a check-by-check pass.
3. **Negative controls every gate.** Several browser checks rerun their whole walk against a broken
   copy of the page once per control (column-overscroll: six control walks beside two real ones).
   They prove the check can see each defect; they protect against a change to the check or the
   page. Running them only when the check or page changes (or nightly) would cut those checks by
   half or more, at the cost of a control rotting unnoticed for a while.
4. **`build.tests`: 1 316 s CPU.** 89 integration-test binaries, each linking the whole crate
   in release mode, plus every dependency rebuilt because `tokio`'s `test-util` dev-feature changes
   `tokio`'s features and so every crate built on it. Folding the integration tests into a few
   binaries is the standard fix and a large mechanical change; unifying the features would change
   the shipped binary. Neither is a test-time call to make alone.
5. **The page suite's collection floor is 294 for a file of 2 291 tests.** It was stale before this
   work (it lived in `tests/voice_page.rs`) and moved unchanged; raising it is a policy call.
6. **The `#240 reading-bookmark` 1 100-message tests** (15 s each after): see section 1.

## 5. Pruned

Nothing. No test found here duplicates another's signal outright; the candidates above (controls,
fixed waits) are about running the same signal cheaper or less often, which is the owner's call.
