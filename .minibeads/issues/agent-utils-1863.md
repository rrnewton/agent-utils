---
title: 'vibe-talk-run-sh-mocks-bind-port-zero: every mock run.sh starts binds port 0 and reports its bound port back'
status: closed
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

## Resolution

[watermelon-full-preview] Landed on main as f9688c88 (gate pass on b573f74c, ledger verdict pass, finished 2026-11-19T00:57:36Z; overlap on screenshots.py verified — replayed tree identical to the checked tree). Every mock run.sh starts now binds port 0 and reports its bound port back: the ElevenLabs mock gained `--ports-file` (with a new `MockHandle::http_addr()`), the model stub gained `--port-file`; run.sh starts both on port 0, waits on the port files, readiness-polls the learned ports, and only then writes the server config with the real `api_base`/`base_url`. The P+1/P+2/P+3 derivation and its refusal checks are gone; the server port keeps its claim. Also in this change, per coordb: keyboard-open shots focus the field under test first (`shrink_keyboard` focuses the composer or the Discuss box, scrolls it into the shrunk viewport, and fails the run by name when it lands outside; new matrix state 77-composer-keyboard-open; scenes 75/76 checked — 76 now focuses the Discuss box), and every screenshot run writes `BUILD.txt` (commit, commit-date, page-build) into the shot folder. Overlap note: mu-flakes3's dc86e5fa implemented the same mock pattern independently and was dropped in favour of this one; their JSON ports-file protocol is equivalent to the text one landed here. Evidence: two concurrent screenshot runs under a squatter on 18091-18094 both passed (mocks on kernel ports); matrix 20/20 (73-77, both themes, android-412 + laptop-1280); self-test 62 controls; local validates green (181 passed). The first gate's two reds (keyboard-browser, scroll-stability-browser) pass in all local validates and in exclusive --selected reruns (both EXIT 0) — contention on the loaded host, not this change.
