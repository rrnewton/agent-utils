---
title: 'rust-only-functional-check: prove agent-utils works with cargo and rustc and no python3'
status: open
priority: 1
issue_type: task
labels:
- rust-primary
created_at: 2026-11-22T17:20:00-05:00
updated_at: 2026-11-22T17:20:00-05:00
---

# Description

The owner asked whether everything in agent-utils works with only cargo and rustc installed and no python3. Today the rust-only meter (#542, scripts/check_rust_only.py and validation/rust-only/) only runs each dispatched tool's --help with Python scrubbed from PATH, which proves the binaries start, not that the tools work.

# Design

Extend the hermetic rust-only container check into a functional smoke: in an image with cargo and rustc and no Python interpreter at all, build the workspace and run each Rust-primary tool through a real basic flow (agentctl start/send/stop against a fake harness, dagrun running a small DAG, tick-hub one tick, herdr-run a command, cpuset-alloc reserve and release, pr-landing-planner a fixture plan, the wrkslots readers routed to Rust), reporting pass or fail per tool and a single overall verdict. Python-primary tools are listed as not-yet with their tracking issue. This becomes the Rust-primary campaign's headline metric.

# Acceptance Criteria

- One command produces a pass or fail per tool and an overall answer to "works with no python3"; it runs as a non-gating scheduled check (or gating once green); the README states the current answer.

# Notes

Owner request; filed by the coordinator. Owned by the Rust-primary campaign lane.

[watermelon-full-preview] v1 implemented 2026-11-22 (mu-rustwrk, commit
41ac3fdb on mu-rustwrk/2131-rust-only-smoke, gating next): Containerfile
ships wrkslotsd/wrkvizd/agent-teams/agent-usage/gh-paced + fixtures;
smoke.sh runs real flows (dagrun DAG, tick-hub gated tick, planner fixture
plan, agentctl registry/inbox round-trip, herdr-run local contract,
wrkslotsd status/doctor over a planted registry, wrkvizd inspect) with
per-tool PASS/FAIL/BLOCKED and one verdict line. Host-verified with Python
scrubbed from PATH: 12 passed, 0 failed, 1 blocked (cpuset-alloc needs
systemd-run --user, absent in slim environments; passes on the bare host).
The hermetic in-container run is PENDING: base-image pulls are blocked for
agents on this host (fwdproxy allows registry-1.docker.io but denies the
production.cloudfront.docker.com blob CDN for the agent identity); no
allowlist change will be requested by the lane (owner's call, via coordb).
Rerun where images are reachable:
podman build -t agent-utils-rust-only -f validation/rust-only/Containerfile . && podman run --rm agent-utils-rust-only

[watermelon-full-preview] cold-review MUST-FIX pass 2026-11-22
(mu-rustwrk, on mu-rustwrk/2131-rust-only-smoke): verdict semantics
split (Rust-primary functional verdict passes only with zero real
failures; cpuset-alloc BLOCKED now requires the allocator's own
systemd-scope refusal in stderr, anything else is FAIL; BLOCKED is
never a per-tool pass; whole-repo verdict stays NOT YET). Real flows
added for agent-teams (init + check), agent-usage (planted history
readback), gh-paced (offline classify read vs write); herdr-agent is
HELP-ONLY (all operations need a Herdr server) and excluded from the
flow count. Planner (1043 / 987-942 on src/shared.rs /
refire-stale-gate), wrkvizd events, wrkslotsd slot-a-in-doctor, and
agentctl capabilities (herdr + muse) assertions tightened to known
answers. herdr-chat, parallel-experiment-runner, and run-agent-prompt
restored to the not-yet list. Wiring: check_rust_only.py --functional
runs the smoke from the existing rust-only DAG job when release
binaries exist and skips by name otherwise, so gates never need
podman/network; the DAG has no scheduled/non-gating concept for
environment-dependent steps, and adding one remains the scheduler
gap for this issue. In-image network needs (docker.io + crates.io,
native deps unverified) and the host-run command are documented in
the Containerfile header. The hermetic podman run and the new flows'
host run remain pending: no release binaries are built on this host
and base-image pulls are blocked for agents, so neither was executed
in this pass; the issue stays open on those two runs.
