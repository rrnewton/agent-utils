# Does the number of idle command tabs explain a slow Herdr server?

`#527 herdr-run-idle-retention`, 2026-10-25. Measurements behind the change that keeps few idle
herdr-run command tabs (`max_idle_tabs`, a shorter `idle_tab_ttl_seconds`, reuse of the least
recently used idle tab).

## What was seen

A shared Herdr server (one long-running process serving several projects' workspaces) held 148
panes. One project's command workspace was at its `max_panes` cap of 32 tabs, 30 of them idle far
past `idle_tab_ttl_seconds`; `herdr-run sweep --idle` closed them at once. The server sat near one
full core and control commands intermittently timed out after 30 s.

The 30 tabs survived because nothing swept them: the sweep at the end of a run examined at most
four expired tabs, and in a workspace whose runs had become rare no run came along to do even
that. The TTL bounded how long a tab could linger only when someone looked.

## The live server after the prune (read-only)

Sampled after the 32 tabs were closed, without changing anything on the server: CPU from
`/proc/<pid>/stat` over 10 s, latency of five calls of each read-only control command.

| | value |
|---|---|
| panes | 122 (one workspace holds 104 of them: agent sessions, most of them finished but never stopped) |
| server threads | 568 (started by hand, so without the `TOKIO_WORKER_THREADS=16` cap herdr-run applies when it starts a server; 316 runtime workers on this host) |
| server CPU, idle-ish | 37–40% of one core (two 10 s samples) |
| `pane list` (all workspaces) | 56–74 ms |
| `workspace list`, `tab list`, `pane process-info`, `pane read` | 3–9 ms |

Of the server's CPU in one 10 s window, the main thread took 8.7% and fourteen runtime workers
2–3.5% each; only two panes changed `revision` in that window.

## A private server, controlled (`bench.py`)

A private Herdr server per configuration (own `HOME`, XDG directories, and socket; the installed
`herdr` 0.8.0, the same build the shared server runs), one workspace filled with N tabs. CPU over
15 s; latency median/max over 15 calls; "8 concurrent" is `pane list` from 8 callers at once.
Raw rows are in `results.jsonl`.

Idle bash tabs, runtime workers uncapped (the shared server's configuration):

| tabs | threads | CPU | `pane list` | `process-info` | `pane read` | `pane list` ×8 concurrent |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 322 | 0.1% | 1.4 ms | 1.4 ms | 1.7 ms | 2.3 ms |
| 32 | 384 | 2.4% | 3.5 ms | 1.6 ms | 1.9 ms | 12.9 ms |
| 64 | 447 | 2.8% | 5.3 ms | 1.3 ms | 1.5 ms | 17.3 ms |
| 128 | 575 | 3.5% | 9.0 ms | 1.6 ms | 1.5 ms | 38.0 ms |
| 192 | 703 | 6.6% | 13.5 ms | 1.6 ms | 1.6 ms | 52.1 ms |

With `TOKIO_WORKER_THREADS=16` the same populations measured 0.1–26.7% CPU and 1.4–28 ms for
`pane list`, noisier than uncapped on a host loaded by other work at the time; the cap is not what
these idle populations need.

Tabs whose terminal keeps changing, 128 tabs, uncapped:

| busy tabs | what they run | CPU | `pane list` | `pane list` ×8 |
|---:|---|---:|---:|---:|
| 0 | idle bash | 3.4–4.3% | 8.8–9.1 ms | 35.5 ms |
| 16 | full-screen redraw, 5 Hz | 9.5% | 12.5 ms | 49.2 ms |
| 64 | full-screen redraw, 5 Hz | 13.5% | 11.2 ms | 57.6 ms |
| 32 | `top -d 0.5` | 12.6% | 10.7 ms | 41.3 ms |
| 96 | `top -d 0.5` | 16.2% | 13.7 ms | 55.8 ms |
| 16, client attached | full-screen redraw, 5 Hz, a TUI client attached and focused on the workspace | 16.6% | 10.9 ms | 43.2 ms |

(Rows from `bench.py tabs`, `bench.py busy` (with `BUSY=32,96 bench.py busy 'top -d 0.5'` for the
`top` rows) and `bench.py client`. The rows in `results.jsonl` were taken with an earlier, untyped
draft of the same script, before it was consolidated; the scenarios and measurements are the same.)

## Conclusions

1. **Idle tab count alone does not explain the slowness.** 192 idle shells cost a private server
   6.6% of a core and 13.5 ms per `pane list`; the cost is linear and small (about 0.07 ms per pane
   per listing). Closing the 32 idle command tabs removed about a fifth of the shared server's
   panes, and the server answered afterwards, but by these numbers those tabs were worth a few
   percent of a core, not the core the server was burning.
2. **The shared server is several times slower per pane than a fresh one with the same build and a
   similar count**: `pane list` 56–74 ms at 122 panes against 9–14 ms at 128 panes on a private
   server, even with 96 panes redrawing and a client attached, and 37–40% CPU against at most
   16.6%. What differs is what the panes hold and how the server runs: 104 panes of agent sessions
   in one workspace (most finished and never stopped, several still animating), an uncapped
   runtime (568 threads; the allocator convoy measured at 260 panes, >1000% CPU, is the failure
   mode of exactly that configuration under concurrent load), and three weeks of uptime. Profiling
   the shared server would say which; that was out of scope for a read-only look.
3. **What to do about the server**, in order of expected effect: stop the finished agent sessions
   in the 104-pane workspace; restart the server with `TOKIO_WORKER_THREADS=16` (what herdr-run's
   own auto-start uses); then measure again. Keeping command tabs few (this change) removes one
   steady contributor and the tail of control calls herdr-run itself makes per tab, but it is not
   the main lever.

## What the change does about the tabs

* `max_idle_tabs` (default 3): past that many idle recorded tabs, the least recently used are
  closed at the end of every run and by `sweep --idle`. Counted from run records, so the tabs kept
  are not probed; a tab with a command in flight is never counted (every run is now recorded as in
  flight the moment it is typed).
* A new agent takes over the least recently used idle tab when the pool is full, under the same
  idle verdict plus a check that the tab and shell are the ones its last run recorded, holding the
  pane lock through its command. A run re-checks its tab's label once it holds the pane lock and
  resolves again once if the tab was closed or handed over in between.
* `idle_tab_ttl_seconds` default 900 → 300, and the end-of-run sweep is the whole sweep, every time.
* A `sweep --idle` user timer recipe in the user guide for hosts where runs are rare.
