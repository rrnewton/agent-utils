---
name: herdr-run
description: Run a cooperatively allowlisted network command through an out-of-sandbox Herdr pane while preserving output, status, readiness evidence, serialization, and an operational audit trail. Use when an agent sandbox blocks a legitimate git, gh, or dependency-fetch operation.
---

# herdr-run

Use this command only for operations admitted by the project policy. Its allowlist is a cooperative
safety rail, not a same-user containment boundary; do not widen policy without reading the trust
model.

The installed CLI is the source of truth:

- `herdr-run --help`
- `herdr-run config` to inspect the fully resolved policy
- `herdr-run check '<command>'`
- `herdr-run --agent NAME --dry-run '<command>'` to validate and render without touching a pane
- `herdr-run doctor`
- `herdr-run userguide`

Dry-runs and refusals still append operational evidence to `.herdr-run/audit.jsonl`. Successful
runs keep `command`, byte-exact `stdout`/`stderr`, `exit_code`, and `meta.json` under
`.herdr-run/runs/<run_id>/`; `--json` reports that path. Completed run spools are pruned on a later
write after the configured retention window (four days by default), while active runs are retained.
