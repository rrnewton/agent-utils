---
title: 'engine-rust-default: make Rust the default engine for paired tools'
status: closed
priority: 2
issue_type: task
created_at: 2026-10-25T12:23:34.017596650+00:00
updated_at: 2026-10-25T14:57:12.735126787+00:00
closed_at: 2026-10-25T14:57:12.735126536+00:00
---

# Description

Owner decision 2026-10-25: flip the repository default from Python to Rust. common/bin/engine-resolver defaults paired tools (agentctl, cpuset-alloc, dagrun, herdr-agent, herdr-run, pr-landing-planner, tick-hub) to the tracked Rust launcher; Python-only tools (herdr-chat, herdr-subagents, parallel-experiment-runner, wrkslots, wrkviz) keep running Python with no override. DAGRUN_ENGINE=python (and the clearer AGENT_UTILS_ENGINE) stay explicit opt-ins; an explicit rust request for a Python-only tool still refuses. Update resolver docs, README/QUICKSTART/site, setup modes and the setup skill, validation and the cross-language differential (both editions exercised explicitly). Python-only subcommands of paired tools get clear Rust-edition refusals naming DAGRUN_ENGINE=python.

# Acceptance Criteria

Resolver defaults verified by tests for paired and Python-only tools; explicit python/rust opt-ins verified; agentctl Python-only subcommands refuse clearly under Rust; make validate-all green on the landed HEAD.
