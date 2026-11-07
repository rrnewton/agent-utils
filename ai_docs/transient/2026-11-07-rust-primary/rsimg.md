# wrkslots image/sandbox Rust-primary parity inventory (milestone 1, P8/P9)

Date: 2026-11-07. Issue: #562 rust-primary-wrkslots, steps P8/P9 (+ P7 image corners).
Lane: mu-rsimg, slot rsimg, branch mu-rsimg/rust-primary-wrkslots-image-sandbox.
Measured at HEAD `230c8f47` (github/main after rebase, 2026-11-07 10:06 EST).
Sibling inventory read first: mu-rswrk's `wrkslots.md` (commit `462a7ed2`;
landed on github/main as `727a6a5f`). Boundary below confirms the
boundary drafted in that landed doc, with one refinement (the four
P7 hooks); no change to shared-file ownership is proposed.

Finish line (shared with mu-rswrk): every wrkslots command is Rust-default
with differential coverage, and the Python edition is archived as #238
describes for agentctl. Until then new behaviour lands in BOTH editions.

Safety boundary for this lane: tests use fixtures and temp images ONLY.
Never touch `worktrees/` or any live slot image; never mount/unmount
anything outside own temp fixtures. Live slot images on this host are
real ext4 images mounted with `sudo -n`.

## Corrections to the brief (verified in code)

- `worktrees-image` has **6 actions, not 5**: `create|mount|unmount|status|grow|trim`
  (`py/wrkslots/imagecmd.py:183-213`), plus top-level `migrate` (:215-227).
- Dispatch for `image`, `worktrees-image`, `migrate`, `run`, `box`,
  `sandbox`, `limits` is **not in `py/wrkslots/cli.py`**: cli.py only calls
  `imagecmd.register(subparsers, _HelpFormatter)` at `py/wrkslots/cli.py:68056`;
  registration and handlers live in `py/wrkslots/imagecmd.py:29-586`.
- `image` actions confirmed as 7: `status|mount|unmount|trim|grow|set-default|convert`
  (`imagecmd.py:47-181`; bare `image` prints help).
- P7 note: Rust `create` has a **fail-open gap**, not just a refusal (below).

## Router truth today (verified in code)

- `validation/editions.json:132`: `wrkslots` primary is **python**, Rust
  mirror state is **absent**, `parity_check: null`. **No wrkslots command
  is Rust-default today** (agrees with mu-rswrk).
- `common/bin/wrkslots-soak:65-86`: soak maps only listed commands to Rust;
  `image`, `worktrees-image`, `migrate`, `run`, `box`, `sandbox`, `limits`
  are unlisted and fall through to `run_python "(not yet in Rust)"`.
  Only family member mapped is `shell-command` (:72).
- `rs/wrkslotsd/src/cli.rs`: no `Image`, `WorktreesImage`, `Migrate`,
  `Run`, `Box`, `Sandbox`, or `Limits` variant. Only adjacent items:
  `ShellCommand` (:227, incl. `--box` flag :236) and image-adjacent flags
  `discard_stale_image_copy` on recover paths (:957) and
  `representation` on create (:1043, :1102).
- Every Rust writer is fixture-guarded (`WRKSLOTSD_FIXTURE_WRITER=1`,
  `ensure_states::require_fixture`); even landed steps stay fixture-only
  until mu-rswrk's P11 staged live flip.

## Sizes (Python left, this lane)

| Module | Lines | Role |
|---|---|---|
| `py/wrkslots/slotimage.py` | 1,859 | per-slot ext4 image: record, provision, mount, grow |
| `py/wrkslots/imagecmd.py` | 1,810 | registration+handlers for all P8/P9 commands |
| `py/wrkslots/imagehelper.py` | 476 | root helper (`sudo -n`, loop ioctls, mount) |
| `py/wrkslots/worktreesimage.py` | 614 | shared btrfs worktrees image + binds |
| `py/wrkslots/sandbox.py` | 2,355 | sandbox settings, spec, namespace/cgroup exec |
| Total in scope | 7,114 | plus cli.py glue (config parse, create/finish/remove hooks) |

Reusable Rust substrate (not commands): `storage_state.rs` (1,279 lines;
`IMAGE.json` validation, mountinfo/loop evidence), `config_validate.rs`
(image :320 and worktrees-image :364 settings, sandbox :620-830),
`shell_command_state.rs` (347; ported, but printed line still names
Python `run`/`box`).

## P8a: `image` (7 actions) — all Rust: NONE

Handlers return `int`; `Refusal` exits 3, journal-busy 75 (`cli.py:68122`).

| Action | Python handler | Privileged ops | Rust | Differential |
|---|---|---|---|---|
| status | `imagecmd.py:865`, `slotimage.usage:1673` | none (stat/statvfs, mount table) | NONE (substrate only) | none |
| mount | `imagecmd.py:1014`, `slotimage.ensure_all_mounted:1543` | `sudo -n imagehelper mount`: LOOP ioctls, `mount(2)` ext4; or `fuse2fs` via `systemd-run --user` | NONE | none |
| unmount | `imagecmd.py:1037`, `slotimage.unmount:1353` | helper `umount2(NOFOLLOW)`; FUSE `fstrim`+`fusermount -u` | NONE | none |
| trim | `imagecmd.py:1063`, `slotimage.trim:1708` | helper `FITRIM` ioctl / `fstrim` | NONE | none |
| grow | `imagecmd.py:1071`, `slotimage.grow:1730` | truncate, `LOOP_SET_CAPACITY`, `resize2fs`; offline `e2fsck` | NONE | none |
| set-default | `imagecmd.py:1085` | none (config write + `.bak`) | NONE (validation exists) | none |
| convert | `imagecmd.py:1123`, `_convert_to_image:1155`/`_convert_to_worktree:1183` | inherits provision: `mkfs.ext4`, mounts, `cp -a --reflink=auto` | NONE | none |

Python tests: `test_slot_images.py` (1,100 lines, pure tests always run,
mount tests skip without sudo/FUSE); shell e2e `e2e_images.sh` etc.
No test calls `_cmd_image_*` directly; no Rust differential exists.

Port sketch: new modules `image_record.rs` (IMAGE.json type, promoted
from `storage_state.rs:155`), `image_mounts.rs` (reuse storage_state
mount/loop evidence), `image_ops.rs` (privileged seam: first slice keeps
`sudo -n -- python -I imagehelper.py` as executor, argv contract
`slotimage.py:631`; native helper is a later safety-critical step because
the crate is `#![forbid(unsafe_code)]` in `lib.rs:42`), `imagecmd.rs`.

## P8b: `worktrees-image` (6 actions) + `migrate` — all Rust: NONE

Common: `_wt_settings` (`imagecmd.py:703`) refuses unconfigured;
`WorktreesImageError` exits 3. Privilege seam `_run(sudo=True)`
(`worktreesimage.py:210`). Hidden side effect to NOT reproduce blindly:
`_load_config` auto-runs `ensure_image_mounted`+`ensure_binds` on every
load (`cli.py:4062`), so even `status` may `sudo mount` in Python.

| Action | Python handler | Privileged ops | Rust | Differential |
|---|---|---|---|---|
| create | `imagecmd.py:715` → `create_image:219` | `mkfs.btrfs` (sudo), chattr, truncate | NONE | none |
| mount | `imagecmd.py:725` → `ensure_image_mounted:190` | `sudo mount -o loop,...`, btrfs quota, bind mounts | NONE | none |
| unmount | `imagecmd.py:741` → `unmount_image:273` | `sudo umount` (does not unbind first) | NONE | none |
| status | `imagecmd.py:751` → `status_report:599` | read-only itself (see auto-mount above) | NONE | none |
| grow | `imagecmd.py:766` → `grow_image:279` | truncate, `losetup -c`, `btrfs resize max` | NONE | none |
| trim | `imagecmd.py:776` → `trim_image:302` | `sudo fstrim -v` | NONE | none |
| migrate | `imagecmd.py:786` | btrfs subvolume create, copy+verify, bind over original (original kept beneath) | NONE | none |

Python tests: `test_worktrees_image.py` (538 lines, 20 tests; pure
settings/classify/signature/record + `_FakeMounts`-injected binds; real
mkfs/mount not automated). Rust overlap: config validation only
(`config_validate.rs:364`).

Port sketch: `worktrees_image.rs` (pure: settings reuse, `classify_mount`
over injected mount table, `listing_signature`, bind records) +
`worktrees_image_write.rs` (single Runner seam for sudo argv).
Decision needed: Rust `status` should classify without mounting;
explicit mount only in `mount` handler.

## P9: `run`, `box`, `sandbox`, `limits` — Rust: NONE (except shell-command)

Registration: `imagecmd.py:229-586`; `_view_arguments:589-662` shared flags.

| Command | Python handler | Privileged ops | Rust | Differential |
|---|---|---|---|---|
| run | `imagecmd.py:1435` → `sandbox.run:2025` | user systemd slices (`systemctl --user`), D-Bus scope-in-place, userns `unshare`+mounts, or root helper via `sudo -n` | NONE | none |
| run --place-only / --in-scope | `imagecmd.py:1492` / `:1562` | slice limits; in-place scope entry | NONE | none |
| box (coordinator) | `imagecmd.py:1417` → `box_view:1353` | same as run; slice `<root>-box-<name>.slice`; `.agentctl` writable exception | NONE | none |
| sandbox show-config | `imagecmd.py:1622` | none | NONE (validation reusable :620) | via validate-config only |
| sandbox write-defaults | `imagecmd.py:1628` | none (locked config write) | NONE | none |
| limits show/apply/clear | `imagecmd.py:1727` | `sudo -n systemctl set-property --runtime user-<uid>.slice` (apply/clear) | NONE | none |
| shell-command (adjacent) | `imagecmd.py:1652` | none (prints exec line) | **FULL** (`shell_command_state.rs`) but printed line still names Python `run`/`box` | `test_shell_command_cli_rust.py` (5 tests) |

Key facts: sandbox never touches the network (`sandbox.py:40`:
network explicitly untouched; `test_network.py` is the git proxy
wrapper, not sandbox). `run`/`box` exec in place (PID unchanged via
D-Bus scope, fallback `systemd-run --scope`). Exit: boxed command's
status is the process status; `SandboxError` in helper exits 126.

Python tests: `test_cli.py` ~30 sandbox tests with fakes,
`test_sandbox_cpu_quotas.py` (6 pure tests), shell acceptance scripts
(`accept_coordinator_box.sh`, need systemd/sudo, environment-gated).
No `cross/` wrkslots differential exists at all (mu-rswrk confirms);
the per-command `test_*_rust.py` file pattern is the coverage norm.

Port sketch: `sandbox_state.rs` (pure 80%: settings/limits, slice
naming from fixture cgroup text, `build_spec`/`writable_set`/
`mask_report`/env, `verify_in_scope` — all path/text-injected),
`sandbox_exec.rs` (thin privileged: systemctl/busctl/systemd-run,
sudo root-spec, mount-namespace builder behind `#[cfg(linux)]`),
`limits_state.rs` (fraction/property computation + systemctl seam),
hidden `sandbox-enter`/`sandbox-enter-root` helper subcommands so the
Rust binary is self-contained (changes `shell_command` printed line —
coordinated cutover with mu-rswrk).

## P7 image corners of create/finish/remove (coordinate with mu-rswrk)

- Create: Rust refuses explicit image at `create_write.rs:421-424`
  ("slot-image provisioning stays with the Python edition"), BUT
  `representation=config` resolving to `slot_representation: image`
  is **not refused — Rust silently creates a plain worktree**
  (no resolved-representation read; `create_write.rs:872`). Also no
  worktrees-image subvolume+bind birth, no `--image-ceiling-gib`.
  This fail-open gap is the first P7 differential to add.
- Finish: Rust refuses on mere `IMAGE.json` existence
  (`finish_write.rs:1285-1294`), broader than Python, which only
  refuses when unmounted (`cli.py:24688` gate). A read-only Rust
  IMAGE.json parser already exists (`storage_state.rs:109`).
- Remove: no direct Rust image check; refusal is transitive via
  `finish_write::assert_slot_contents` (`remove_write.rs:1567,1747`),
  so the message mislabels it as finish. No `--discard-stale-image-copy`
  on Rust `Remove`.
- Python image paths stay in `_cmd_create` (`cli.py:27964`: provision
  :28116, subvolume birth :28098), finish gate (:24653), remove
  teardown (`_remove_slot_directory`, `slotimage.destroy`,
  `worktreesimage.unbind_slot`).

### Proposed written boundary (for mu-rswrk agreement via coordb)

mu-rsimg owns representation mechanics; mu-rswrk owns lifecycle sequencing.

- mu-rsimg owns new Rust modules mirroring `slotimage.py`,
  `worktreesimage.py`, `imagecmd.py`, `imagehelper.py`: IMAGE.json
  schema, `slot-images/` layout, provision/mount/unmount/relocate/
  destroy, mount-table parsing, kernel/FUSE backends, worktrees-image
  subvolume/bind lifecycle, and P8/P9 commands. Inside create/finish/
  remove it owns four decision hooks returning Python-identical
  refusal strings: (1) resolve representation incl. ceilings,
  (2) provision/bind before `git worktree add`, (3) mounted-state
  gate for finish/remove, (4) fence-relocate + destroy/unbind.
  It never writes ACTIVE/ARCHIVED/EVENTS/journals.
- mu-rswrk owns shared core: `cli.rs` dispatch, `create/finish/remove_write`,
  config loading, `ensure_states` fixture gate, state/event/journal
  formats, Git salvage/handoff/fence sequencing, router/editions.
  It calls the hooks at those points and keeps explicit refusals
  until they land.
- Existing IMAGE.json reader in `storage_state.rs` moves under
  mu-rsimg ownership; core consumes it rather than re-parsing.

mu-rsimg adds NEW modules only and asks before editing shared files:
`cli.rs`, `lib.rs`, `formats.rs`, `ensure_states.rs`,
`validation/editions.json`, `common/bin/wrkslots-soak`,
`common/bin/engine-resolver`, `validation/components.json`/DAG.

## Differential test strategy (privileged ops without root)

Principle: decisions are pure; effects go through one injectable Runner.

1. Pure/fixture first (no root): fake Runner records argv (analogue of
   Python `_FakeMounts`); injected mountinfo/cgroup text; temp
   `slot-images/` trees with fake `IMAGE.json` + sparse files;
   `mkfs.ext4`/`mkfs.btrfs` on temp files is unprivileged and allowed;
   real mounts are never differential prerequisites.
2. Golden `--print` differential: `run --print`, `box --print`,
   `run --in-scope --print`, `--place-only --format json` with fake
   `systemctl`/`busctl`/`sudo` shims on PATH + fixture project;
   compare stdout/stderr/exit byte-for-byte.
3. Capability-probed smoke (skip, not fail): unprivileged userns
   mount-outcome probe in a temp slot; `root`/`limits apply` and
   loop-mount lifecycle stay acceptance-only (existing shell scripts).
4. New tests follow the norm: `py/wrkslots/tests/test_*_rust.py`
   cloned from `test_shell_command_cli_rust.py` via `rust_binary.py`,
   registered in `validation/components.json` (fail-closed inventory).
5. P7 first tests (fixture IMAGE.json, no mounts): create explicit
   image refusal, create config→image fail-open repro, finish/remove
   target-image preservation — both engines, ACTIVE unchanged.

## Ordered port plan (agent-days incl. differential tests)

- S1. P7 hooks + fail-open fix: representation resolution, mounted
  gate, IMAGE.json promotion to `formats.rs`. M (3 d).
- S2. `image status` + `set-default` (pure/read+config write). S (2 d).
- S3. `worktrees-image status` + settings/classify parity. S (1.5 d).
- S4. `sandbox show-config/write-defaults` + `limits` (pure+seam). S (2 d).
- S5. `image mount/unmount/trim/grow` via helper seam. M (4 d).
- S6. `worktrees-image create/mount/unmount/grow/trim` + `migrate`. M (4 d).
- S7. `image convert`. M (3 d).
- S8. `sandbox_state` pure spec/slice/env parity (golden --print). M (4 d).
- S9. `run`/`box` exec (userns + scope-in-place) + shell-command
  printed-line cutover. L (6 d).
- S10. Privileged helper natively (loop ioctls) — only if coordb
  approves unsafe/rustix scope; else helper stays Python-executor
  and is archived last. L (5 d, optional).

Each step: Rust impl, differential test, soak mapping extended,
router default flipped per-command only once parity is proven
(mu-rswrk owns the flip mechanics; mu-rsimg proposes).

## Landing sequence (clock ETAs, America/New_York)

Fits mu-rswrk's landed plan: already-FULL commands Rust-default by
2026-11-14 (mu-rswrk), **P8/P9 by 2026-12-06**, full Rust-default
2026-12-10, Python archived 2026-12-15. This lane is fat: S2/S4 run
in parallel with S1, S5/S6 in parallel, S8 in parallel with S5-S7.

- M1 inventory (this doc): 2026-11-07 12:00 ET.
- Boundary confirmed to mu-rswrk + coordb: 2026-11-07 (this report).
- S1 P7 hooks + S2 (image status/set-default): 2026-11-12 17:00 ET.
- S3 (worktrees-image status) + S4 (sandbox show/limits): 2026-11-16.
- S5 (image mount family): 2026-11-21. S6 (worktrees-image/migrate): 2026-11-25.
- S7 (image convert) + S8 (sandbox pure parity): 2026-11-29.
- S9 (run/box exec + shell-command cutover): 2026-12-06 17:00 ET.
- S10 helper decision point: 2026-12-08 (inside full-default 12-10).

Gate per landing: `python3 scripts/validate.py --all --keep-going`
on the exact push tree (bare, never piped), bare
`scripts/check_client_names.py`, `cargo fmt --all --manifest-path
rs/Cargo.toml -- --check`. Known load flakes rerun alone and shown
passing; failures in touched code are real. Land via `land-main`;
report each 40-hex SHA to coordb only after it exits 0.

Coordination: another team's lane lands wrkslots fixes in
`py/wrkslots/cli.py` often; rebase onto github/main frequently and
port its new changes to Rust in the same step. No isolated subagent
worktrees from this lane; any that appear are removed with
`git worktree remove` as soon as their subagent finishes.
