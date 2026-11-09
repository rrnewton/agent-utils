---
title: 'init-repair-version-gate-after-write: wrkslots init --repair writes the repaired config before checking min_wrkslots_version, then refuses'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-09T11:22:38.000000000+00:00
updated_at: 2026-11-09T14:00:00.000000000+00:00
---

# Description

`wrkslots init --repair` persists the repaired config and only then checks
#1109's min_wrkslots_version gate, so it refuses after it has already changed
the file.

Repro (from a consumer's reviewer, relayed by the lead): a scratch schema-2 config
with min_wrkslots_version 99.0.0 and max_active_slots 1. Running
`wrkslots --machine probe init <scratch> --liveness-command live.py
--max-active-slots 2 --repair` prints "REPAIRED" with the cap going from 1 to 2,
persists cap 2, then exits 3 naming required 99 against current 0.7.

In py/wrkslots/cli.py, _cmd_init writes the config before it calls
_load_config, which is where the version gate runs. The reviewer reports that
be8b3a8e refused before writing, so this regressed between be8b3a8e and
cc487cc3. The likely source is d4eb6ae5 (#1109 wrkslots-config-forward-compat).

# Design

- Check the version gate (and any other refusal that does not depend on the
  write) before any write in init --repair and init.
- Audit the other config writers (config set or similar, migrate) for the same
  order.
- Rust: if Rust has init --repair, give it the same order and a differential
  test.

# Acceptance Criteria

- On a version-gate refusal, init --repair changes no bytes of the config (a
  test compares the file before and after).
- Other config writers audited; any with the same order are fixed or listed in
  Notes.
- Both editions where Rust implements init --repair.

# Notes

Not on the consumer's run path (it has no init --repair caller), so it does
not block its version bump. Reported 2026-11-09 via the lead. Filed by the
devbig030 coordinator. Goes to cl-wrkid (author of #1109) after #1121 and #1122.

## Resolution

Fixed (cl-wrkid, 2026-11-09). The version check from #1109 is now its own function, `_assert_min_wrkslots_version`, which `_forward_compatible_config` still calls for every read. `_cmd_init` calls it on the existing configuration right after reading it, before it compares, repairs or writes anything. So `init` and `init --repair` refuse ("requires wrkslots X or later") with the file byte for byte unchanged and no REPAIRED line. Cause: since #1109 the check ran only inside `_load_config`, which init reaches after its write. Before #1109, `min_wrkslots_version` was simply an unknown key, refused earlier.

Test: `test_init_too_old_for_the_configuration_changes_no_byte_of_it` (repair and plain init), the reported case (min_wrkslots_version 99.0.0, max_active_slots 1 to 2), comparing the file's bytes before and after. On the previous code it fails exactly as reported: REPAIRED, then the version refusal.

Other config writers audited: `config convert` loads, and so version-checks, before it writes; init's fresh-file branch has no existing file; `_recover_config_write` (still before the check) only completes an interrupted write of init's own, so it adds no new content. No other writer of `.wrkslots.yml` exists. The Rust edition has no `init` (it is in Rust's Python-only command list), so there is nothing to port.

