---
title: 'wrkslots-config-forward-compat: an older wrkslots refuses every new optional config key, so each one is a fleet-wide flag day'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-09T03:05:00.000000000+00:00
updated_at: 2026-11-09T04:10:00.000000000+00:00
closed_at: 2026-11-09T04:10:00.000000000+00:00
---

# Description

Unknown top-level keys in `.wrkslots.yml` hard-refuse (`_load_config` -> `_exact_keys` -> "configuration has invalid fields: unknown <keys>"). So every new optional key (`identity` from #1096, `reclaimable_ignored_paths` from #1088) can be used only once every reader of that config has been upgraded.

On one consumer host, 23 owner-live slots (20 agent, 3 validate) still run older agent-utils pins, and its launcher points every slot at the one shared config in the primary checkout. Adding either key would make every old-pin slot refuse, even read-only `status`. The consumer cannot roll out #1096 or #1088.

# Design

The lead's design, which keeps typos failing closed:

1. A top-level `ignorable_keys: [identity, ...]` list. A reader that doesn't know a key ignores it, with a one-line warning naming the key and the reader's version, ONLY if the config lists that key in `ignorable_keys`. An unlisted unknown key still refuses, so a typo like `max_active_slot` is never silently ignored.
2. A `min_wrkslots_version` (or the existing schema integer) that writers bump only when a key is load-bearing for correctness. An older reader then refuses naming the required version, instead of the generic invalid-fields error.
3. Both new fields are understood from this release on: one last flag day, after which additive keys roll out incrementally.

Document the rule in the config reference ("additive optional key: list it in ignorable_keys; correctness-bearing: bump min_wrkslots_version"). Mirror it in Rust (config_validate.rs).

# Acceptance Criteria

- Tests with an old-key-set reader fixture, in both directions: a listed unknown key is ignored with the warning; an unlisted unknown key refuses; a typo refuses; `min_wrkslots_version` above the reader's version refuses naming the version.
- Python and Rust agree (differential test).
- The config reference documents the rule and both fields.

# Notes

Requested by the lead (devbig014), 2026-11-08, from a consumer's rollout blocker. Sequenced after #1108 and ahead of #1106/#1107. Related: #1088, #1096.

## Resolution

Delivered by cl-wrkid, 2026-11-08 ET, in both editions (py/wrkslots/cli.py `_forward_compatible_config`; rs/wrkslotsd/src/config_validate.rs `forward_compatible`).

- `min_wrkslots_version: MAJOR.MINOR.PATCH` is checked before the exact-key check, so an older reader refuses naming both versions even when the file also has keys it does not know: "configuration <path> requires wrkslots X or later (min_wrkslots_version); this is wrkslots Y. Run a newer wrkslots for this project". The reader's version is the Python package's `__version__`. This release bumps it to 0.7.0, and Rust carries the same value as `WRKSLOTS_VERSION`; the differential checks that both name the same version.
- `ignorable_keys: [name, ...]` lists key names (lowercase, digits and `_`, no duplicates). A reader drops each listed key it does not know, with one warning per key and process: "WARNING: wrkslots <version> ignores configuration key <key> in <path>: it does not know the key, and ignorable_keys lists it". Everything else still goes through the exact-key check, so an unlisted unknown key or a typo still refuses. A listed key that the reader does know is used as usual. `init --repair` keeps listed unknown keys.
- Both keys are documented in the literate config comments (both editions' tables) and in USER_GUIDE "Configuration file" / "New keys and older readers". The guide states the rule: an additive key needs an `ignorable_keys` entry; a correctness key needs `min_wrkslots_version` raised to the release that introduced it, which means bumping `__version__` in that release. It also records the last flag day: readers before 0.7.0 refuse both new keys, so use them only once every reader is 0.7.0 or later.
- Tests (py/wrkslots/tests/test_config_validate_rust.py, Python and Rust differential):
  - a listed unknown key is ignored with the same warning by both editions;
  - an old-key-set reader fixture (Python, `identity` removed from the known keys) ignores a listed `identity`, while the same file read by this build uses it silently;
  - an unlisted unknown key and a typo refuse in both editions;
  - malformed `ignorable_keys` and versions refuse in both;
  - a newer `min_wrkslots_version` refuses with the identical message in both.

