---
title: 'vibe-talk-run-sh-mocks-bind-port-zero: every mock run.sh starts binds port 0 and reports its bound port back'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T15:10:00-05:00
updated_at: 2026-11-18T15:10:00-05:00
---

# Description

[watermelon-full-preview] Concurrent gates keep failing screenshot steps because `vibe-talk/scripts/run.sh`'s offline ElevenLabs mock binds a chosen loopback port that something else already holds ("could not bind 127.0.0.1:48354: Address already in use"; also seen on 37716). It cuts several flaky gates an hour.

The screenshot port claims (#1037, extended to four ports by #1855) only exclude other claiming runs: a non-claiming process (another lane's fixture, a leaked mock, any test binding an ephemeral loopback port) can take a candidate port between the free-port probe and the bind, and the kernel-picked candidate bases (48354, 37716) collide exactly that way. The probe-then-bind window is the defect; widening the claim cannot close it.

# Design

- Every mock `run.sh` starts binds port 0 and reports its bound port back to `run.sh` by a handshake (a port file in the run's throwaway directory, or a parsed stdout line): the ElevenLabs mock's HTTP and WebSocket ports and the loopback model stub. The fake chat upstream is in-process in the server and has no port of its own.
- `run.sh` starts the mocks before writing the throwaway server config, learns the real ports from the handshakes, and writes those ports into the config (vendor `api_base`, model `base_url`) and the screenshot walk's `--mock-control`. Nothing probes a port and then binds it later.
- The server's own port keeps the existing claim mechanism (it is not a mock; the walk needs a port chosen before the server starts), and the claims stay as they are.
- The mock binaries learn to accept port 0 and to write their bound ports to a file when asked; a bind failure still dies by name with the mock's own output.

# Acceptance Criteria

- With a squatter holding the old candidate ports (18091-18094 and a kernel-picked candidate base), a screenshot run still starts its mocks and server and completes.
- Two concurrent `run.sh --screenshots` runs (a fast fixture subset is enough) both start and complete without a bind collision.
- The standard matrix (states 73-76, both themes, android-412 and laptop-1280) still passes, and the screenshot self-test and `scripts/test-run-sh.sh` stay green.
- No probe-then-bind remains for any mock `run.sh` starts: each mock's bind is the port-0 bind itself.

# Notes

Gate infrastructure, not owner UI: gate normally, no owner priority. Follow-up to #1855 vibe-talk-screenshot-harness-real-states, whose four-port claim this replaces for the mocks.
