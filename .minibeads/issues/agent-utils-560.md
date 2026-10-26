---
title: 'package-contract-oom: packaging step OOM-killed at its 640MiB boxed cap under host load'
status: closed
priority: 2
issue_type: bug
labels:
- validation
created_at: 2026-10-26T21:59:46.301522203+00:00
updated_at: 2026-10-26T23:16:13.065351812+00:00
closed_at: 2026-10-26T23:16:13.065351702+00:00
---

# Description

[watermelon-full-preview] Found while gating #552 wit-tts-provider (2026-10-26). The boxed step python.repository-infrastructure.package-contract (cmd: cd py && PYTHONPATH=../scripts python3 -m pytest -p run_pytest_shard -q tests/test_packaging_infrastructure.py; rss_baseline_bytes 536870912, inner MemoryMax ~640MiB) is OOM-KILLED (4 oom_kill events) in its normal boxed form on devbig030 at host load 40-184. Reproduces identically on untouched origin/main (boxed, alone via dagrun --selected, clean tree, 7+ tries), so it is not caused by the change being gated. The same command passes outside the box (30 passed) and passes intermittently under a plain systemd MemoryMax=640M scope (2 of 3 runs); measured peak sits just under the cap, so the boxed run's extra accounting tips it over whenever the host is busy. cl-resume got it green boxed earlier the same day at lower load. Every vibe-talk change selects this step, so it can block unrelated landings for hours.

# Acceptance Criteria

The boxed package-contract step passes reliably under normal multi-agent host load (cap raised with evidence, peak reduced, or accounting fixed), verified by repeated boxed --selected runs at load; or the step is split so the heavy collection no longer shares one 640MiB box.
