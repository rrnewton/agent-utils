---
name: local-ci-hub
description: Run validation on a dev box with host-wide admission, and read its ledger, receipt, health and landing state. Use when starting or sweeping validation runs, checking what is proven about a commit, or diagnosing admission refusals.
---

# local-ci-hub

Validation on a dev box: run, ledger, receipt, remote, health, land.

The installed CLI is the source of truth:

- `local-ci-hub quickstart`
- `local-ci-hub --help`
- `local-ci-hub --userguide`

Start with `local-ci-hub doctor`: it reports the host-limits file,
every effective limit with its source, live headroom, and gate
visibility. A refusal is no result (exit 75), never a test failure;
a missing host file means conservative built-in limp limits with a
warning on every run, not a refusal.
