---
title: 'slot-bind-status-parse: agentctl start and rebind-slot cannot bind slot owners; the status JSON parse fails'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- wrkslots
created_at: 2026-11-11T16:41:05+00:00
updated_at: 2026-11-11T16:41:05+00:00
---

# Description

[opus 5.5] Since 2026-11-11 about 11:30 AM ET, `agentctl start --slot` cannot bind a new slot's owner on one host, and neither can `agentctl rebind-slot`:

    agentctl: warning: slot edone owner bind refused for mu-edone: wrkslots status printed no JSON: trailing characters at line 1 column 8; the launch keeps running unbound - redo it with `agentctl rebind-slot mu-edone`
    agentctl: slot rebind refused: wrkslots status printed no JSON: trailing characters at line 1 column 8

Every slot created that day shows `owner=indeterminate` in `wrkslots status` (four so far). Unbound owners break liveness and TTL decisions, so those slots can't safely be removed or retired until they are bound.

## What was run

- agentctl: the tracked Cargo launcher `agent-utils/bin/agentctl` (engine=rust, `rust cache=verified source=3bfda4df5008 binary=c0e0435cf2ef`), built from a harness checkout at a5296c58. Called as `env AGENTCTL_WRKSLOTS_BIN=<harness>/worktrees/wrkslots agentctl rebind-slot mu-edone`.
- Call site: `slot_row` and `slot_row_document` in rs/agentctl/src/restore.rs, which run `<wrkslots> --project-root <harness> --wait-lock 30 status --slot edone --format json` and `serde_json::from_str` its stdout.
- wrkslots: the harness wrapper is a shell script that runs `WRKSLOTS_ENGINE=rust-soak WRKSLOTSD_FIXTURE_WRITER=1 exec agent-utils/common/bin/wrkslots "$@"` (the #562 soak). `status` runs in the Rust soak edition (`[wrkslotsd] rust cache=verified source=3bfda4df5008 binary=5a5f2fc0693a`; `--version` falls back to Python wrkslots 0.7.0).

## What was found

- The soak edition prints two banner lines, `[wrkslots] status: rust (soak)` and `[wrkslotsd] rust cache=verified ...`, on **stderr**, not stdout.
- Run by hand with the exact argv, the command exits 0 and its stdout is 17,881 bytes of valid JSON (Python `json.loads` accepts it; it starts with `{` and the active row for the slot is present).
- With `AGENTCTL_WRKSLOTS_BIN` pointed at a wrapper that records argv, stdout and stderr and then replays them, agentctl made exactly one call, with the argv above. The recorded stdout was the same valid 17,881-byte JSON, and agentctl still reported "trailing characters at line 1 column 8".
- So the banner theory is probably wrong. A likely cause is how agentctl captures a child's output larger than 16 KiB (out-of-order or interleaved pipe reads in its host runner): a parse that starts partway through the document would fail at line 1 column 8. Not verified.

# Design

- Find why `slot_row_document` sees different bytes from what the child wrote: the host runner's stdout and stderr capture with a timeout, for large outputs.
- Fix it in both editions where it applies, and make the error print the first bytes it got, so the next parse failure is self-explaining.

# Acceptance Criteria

- `agentctl start --slot S` binds the owner (`wrkslots status` shows a live owner) when `wrkslots status --format json` prints more than 16 KiB (and more than 64 KiB), with the soak banners on stderr.
- `agentctl rebind-slot NAME` binds the existing unbound slots.
- A test drives the bind through a fake wrkslots that prints a large JSON document plus stderr noise.

# Notes

Workaround until fixed: the slots run fine unbound, but none of them is removed or retired until its owner is bound (lead, 2026-11-11). The Rust wrkslots lane takes this.
