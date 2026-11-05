---
title: 'vibe-talk-screenshots-port: the screenshot step picks a free port instead of aborting on 18091'
status: closed
priority: 2
issue_type: bug
assignee: cl-start
labels:
- vibe-talk
created_at: 2026-11-05T13:53:16.629288411+00:00
updated_at: 2026-11-05T13:57:20.000000000+00:00
---

# Description

The `vibe-talk.shots.screenshots` gate step hard-codes 127.0.0.1:18091 for its screenshot server. When a live process already holds that port, the step aborts the whole DAG run. On the lead host a long-running vibe-talk test stack holds 18091, so `make validate` cannot pass there at all, and at least one lane is blocked from landing.

# Design

- An override already exists: `VIBE_TALK_SHOTS_PORT` (vibe-talk/scripts/run.sh ~402). Keep that name; do not add a second variable. When it is unset and the default port is busy, the step picks a free port automatically (bind to port 0 and read it back).
- Pass the chosen port to the screenshot server and to every client in the step (scripts/screenshots.py and anything else that reads 18091).
- A busy port must never abort the DAG. If no port can be had, fail the step alone, with a message naming the holder if it is cheap to find.
- Search for other hard-coded fixed ports in gate steps and fix them the same way, or list them in the issue's resolution.

# Acceptance Criteria

- With a process listening on 127.0.0.1:18091, the screenshots step passes on another port, and the DAG completes.
- `VIBE_TALK_SHOTS_PORT` is still honoured.
- No other gate step depends on a fixed port, or each remaining one is listed with a reason.

# Notes

Reported by the lead on 2026-11-05 at 08:51 AM ET (P2; it currently blocks landings validated on the lead host). Assigned to cl-start ahead of #852 C.

## Resolution

[opus 5.5] cl-start, 2026-11-05. Scope per the lead: keep `VIBE_TALK_SHOTS_PORT` as the only override, and pick a free port only when the default is busy.

- **The fix:** with no `--port` and no `VIBE_TALK_SHOTS_PORT`, `scripts/run.sh --screenshots` still uses 18091 to 18093 when all three are free.
  - When any of them is held, it takes three consecutive free ports on 127.0.0.1. The first is picked by the kernel (bound to port 0); the next two are proved free by binding them too. 8080 and 18081 are never used.
  - The server gets P. The offline ElevenLabs mock gets P+1 and P+2. `scripts/screenshots.py` gets `--url` and `--mock-control` for them, as before.
  - A busy default therefore no longer fails the `shots.screenshots` step, or the validation run.
- **Explicit ports:** a port given with `--port` or `VIBE_TALK_SHOTS_PORT` is still refused when taken, naming it. That fails this step alone.
- **If no ports can be had:** a host with no three free consecutive ports in 64 tries fails the step with a message saying so.
- **Tests (`scripts/test-run-sh.sh`, all 83 checks pass locally):**
  - 18091 is used when it and the two ports above it are free.
  - With 18091 held, `--screenshots --dry-run` exits 0 and plans another port.
  - `VIBE_TALK_SHOTS_PORT` is honoured.
  - A held port that was asked for is refused, naming it.
  - That suite runs as `make container-smoke`, not in the gate. The gate's own `vibe-talk.shots.screenshots` step runs the new code.

Other fixed ports reachable from gate steps:
- None that bind. As `vibe-talk/tests.dag.yaml` records, every other browser check binds port 0. send-order and pwa-installability probe a free port just before starting the real server.
- The remaining literal ports in tested code are URL strings that nothing binds or connects to:
  - relay and config parser fixtures (9000, 9100, 18080, 18765, 18771, 41000);
  - `tests/fallback_chain.rs`'s `local_tts` endpoint, served by the in-process `FakeLocalTts`;
  - help text and examples (8080, 8765, 18092).
- `scripts/test-run-sh.sh` publishes decoy containers on 18095 and 18096. It runs only as `make container-smoke`, outside the gate, so they are left as they are.
- `screenshots.py --self-test`'s `http://127.0.0.1:18091` is a probe URL for an offline page stub and opens no socket.
