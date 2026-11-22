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
