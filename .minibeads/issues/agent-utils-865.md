---
title: 'gate-node-provision: run vibe-talk page and contract steps on a provisioned Node 22'
status: closed
priority: 1
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T04:10:15.606607960+00:00
updated_at: 2026-11-05T05:06:17.231374044+00:00
closed_at: 2026-11-05T05:06:17.231373773+00:00
---

# Description

The vibe-talk `page-0` to `page-3` steps and `setup.contract-tools` fail on hosts whose system `node` is older than 18. On the lead host, `/bin/node` is v16.20.2. There:
- `node --test` has no `--test-reporter`;
- esbuild needs Node 18 or later;
- `npm ci` warns EBADENGINE.

Every landing gated on that host currently fails these steps and needs a waiver. Three waivers were needed on 2026-11-04 alone. This is P1 because it blocks every landing gated there.

The gate already provisions its own Node for the timeline browser checks:
- `setup.node22` runs `scripts/provision-node22` (the official archive, sha256-verified, cached per user);
- those steps run through `scripts/with-node22`.

The vibe-talk steps never use that toolchain. `setup.contract-tools` runs `scripts/provision-node-packages vibe-talk/contract` with whatever `node`/`npm` is on PATH, and the page steps call `node` directly.

# Design

- Make `setup.contract-tools` depend on `setup.node22`, and run its npm under `scripts/with-node22`.
- Run every vibe-talk step that calls `node`, `npx` or esbuild (`page-*`, the contract check, and any cargo step whose build.rs drives the page build) under `scripts/with-node22`, or put the provisioned bin directory first on PATH for those steps.
- Fallback, used only when provisioning is truly impossible (offline, and no cached archive): end those steps SKIPPED with a named, visible reason such as `host node v16 < 20; Node 22 not provisioned`, never FAILED. Provisioning comes first, so that the tests still run on such hosts.
- Check `validation/components.json` so the new dependency edge is reflected wherever selection needs it.

# Acceptance Criteria

- With PATH carrying only a Node 16 (or no node at all), `make validate` selecting vibe-talk passes the page and contract steps, using the provisioned Node 22.
- With provisioning forced to fail, those steps report SKIPPED with the reason, and the gate does not report them as passed.
- Hosts with a modern system node behave as today.

# Notes

Requested by the lead on 2026-11-04. Assigned to mu-validate together with the latency-test rework; reassign it to whichever lane frees first if mu-validate is still busy.
