---
title: 'gate-cargo-target-prune: prune cargo target artifacts that the latest gate build did not touch'
status: closed
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T14:30:00.000000000+00:00
---

# Description

Every lane slot keeps its own cargo targets (rs/target and vibe-talk/target), and they grow a few GiB per gate as stale hashed test binaries and incremental dirs pile up. On one host, six live slots held about 125 GiB of targets, and one slot's rs/target went from 5 to 9 GiB in about 6 hours. With many gates an hour, this is the main steady driver of disk use.

# Design

After each gate (or as a gate.py post-step), prune target artifacts that the latest build did not touch: mtime-based, in the spirit of cargo-sweep, keeping everything the just-finished build used. Report bytes before and after. Also evaluate a shared target dir (or a shared build cache) for gate trees, given cargo's target-dir lock.

# Acceptance Criteria

- A gate run reports target bytes before and after the prune; a second identical gate does not rebuild anything the prune removed.
- Stale artifacts from earlier source versions are removed.
- The shared-target option is evaluated with numbers (disk saved against lock contention) in the issue.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.

## Resolution

[opus 5.5] cl-start, 2026-11-06.

**What grows.** Measured on one host's 16 slot target dirs (152 GiB).
- The gate's own release trees stay small: `rs/target/validation-tests` is about 1-2 GiB per slot, and release builds keep no incremental caches.
- The growth is in the debug trees that lanes build by hand (`rs/target/debug`, `vibe-talk/target/debug`).
  - Mostly incremental caches: 12 of 21 GiB in one slot's `rs/target/debug`, and 9.8 of 12 GiB in another's `vibe-talk/target/debug`.
  - Then older builds of the same targets under new hashes.

**Why not plain mtime.** Slots are mounted noatime, and cargo rewrites nothing for a fresh unit, so a file's age says when it was built, not whether anything still uses it. cargo-sweep's time mode reads atime and would delete every long-built dependency here.

**Approach.** `scripts/gate.py` prunes when a run finishes, after releasing its admission slot. Nested gates leave it to their parent.
- **Scope.** A finished gate prunes only its own checkout's `rs/target` and `vibe-talk/target`. `GATE_PRUNE_TARGETS=off` or `--no-prune` turns the prune off.
- **By hand.** `scripts/gate.py prune [--apply] [--other-checkouts] [--keep-hours N] [DIR...]`.
  - It only reports unless given `--apply`, and a report takes no locks.
  - A dir outside the checkout is refused without `--other-checkouts`.
- **Grain.** The unit is the grain, read off cargo's fingerprint graph.
  - Each `.fingerprint/<pkg>-<hash>/` holds the unit's fingerprint (a u64, written little-endian hex) and a JSON record whose `deps` name each dependency's fingerprint. Together these link the units into a graph.
  - A root is a unit nothing depends on.
  - Roots that are the same target in the same configuration (target, profile and mode, features, compile kind, flags) and differ only in what they were linked against are kept:
    - the newest;
    - any built within `--keep-hours` (24) of it with the same compiler;
    - any built in the last hour.
  - Everything a kept root reaches is kept, so a dependency built days ago lives as long as something uses it.
  - Everything else goes: its `deps/`, `build/`, `examples/` and `.fingerprint/` entries.
- **Incremental caches.** These never decide freshness. Per crate the newest stays, and others go once idle for the keep window.
- **Concurrent cargo.** A target dir is pruned only while the prune holds cargo's own locks on every profile dir in it (`.cargo-lock`, `.cargo-build-lock`, `.cargo-artifact-lock`), taken without waiting.
  - cargo 1.99 holds the first shared and the other two exclusive for the whole build, as seen in /proc/locks. A busy dir is skipped and named.
  - An unreadable fingerprint record skips its dir.
- **Reporting.** The gate prints bytes before and after per target dir and writes `target_bytes_before` and `target_bytes_after` to its ledger line. The run's verdict stays the log's last line.

**Measured.**
- A dry run over the 16 target dirs (152 GiB) would free 36 GiB (24%). Examples:
  - one `vibe-talk/target`: 15.5 -> 5.9 GiB;
  - one `rs/target`: 27.5 -> 22.3 GiB.
- Live run on this lane's own slot: `rs/target` 27.58 -> 22.30 GiB in 1.0 s (159 stale units, 306 incremental caches). A second report right after found nothing more to prune.
- The slot is a loop-mounted ext4 image with `discard`, so the host got the space back: the image's allocation went down to 23.9 GiB.
- A dry run over 27 GiB takes under a second.

**Host allocation and trim (follow-up).** Freed blocks reach the host only once the slot image's online discard (or a trim) returns them, so the size of the target dirs alone can overstate the gain.
- The prune now also reports the host's allocation to the slot image before and after.
  - It finds the image through `/sys/dev/block/MAJ:MIN/loop/backing_file`, matched by size across mount namespaces.
  - The ledger gets `host_bytes_before`, `host_bytes_after` and `trim_exit`.
- After freeing anything, it syncs and reads once a second until the allocation stops falling (at most 15 s).
- `GATE_TRIM_COMMAND`, when set, runs once a prune removed something, for example `wrkslots image trim agent SLOT`. It is off by default. Its output and exit status go into the report, and it never fails the gate.
- Measured on this lane's slot (ext4 on loop, `discard`), with a 4 GiB file written, fsynced and deleted:
  - nothing came back for 5 s;
  - all 4 GiB came back by 10 s, which is the journal commit;
  - a sync right after the delete returned about half at 0.3 s.
- So a reading taken seconds after a cleanup can show no gain even though online discard does return everything, which matches a 626 -> 642 GiB jump seen only after a trim.
- End to end, `gate.py prune --apply` removing a 4 GiB stale incremental cache reported host allocation 27.94 -> 23.93 GiB.

**Second identical build.** `py/tests/test_gate_admission.py` covers it with real cargo:
- `dep` is built with and without a feature by `cargo build` and `cargo test`, and a version bump supersedes the first build.
- After the prune, the superseded units and their incremental cache are gone. Both variants and a dependency built "three days ago" remain.
- Rerunning both commands finds every unit fresh. With a zero keep window the test fails, as it should, because the variant gets rebuilt.

**Shared target dir: evaluated, not recommended.**
- **Saving ceiling.** Cross-slot duplicate units on this host (same `<pkg>-<hash>` in two or more slots):

  | Tree | Duplicate bytes | Third-party | Workspace |
  |---|---|---|---|
  | `rs/target/validation-tests`, 6 slots | 4.27 GiB | 1.69 GiB | 2.58 GiB |
  | `rs/target` profiles, 7 slots | 17.5 GiB | 7.4 GiB | 10.0 GiB |
  | `vibe-talk/target`, 9 slots | 18.0 GiB | 8.7 GiB | 9.3 GiB |

- **Workspace units are not really shared.** cargo hashes workspace crates by their path relative to the workspace, so slots at different commits produce the SAME unit hash with different contents. In a shared dir each slot's build overwrites the others', and the next gate in the other slot rebuilds them. So the real ceiling is the third-party part, about 17.7 GiB on this host, roughly 2 GiB per slot, and the workspace part would turn into rebuild churn.
- **Lock contention.** cargo holds `.cargo-build-lock` exclusive for the whole of every build, so all cargo steps of concurrent gates (2 shared slots) and of lanes' own builds would serialize on one lock. A gate's Rust steps run for minutes each (agentctl 388 s, chat-subscription-slack 204 s, agent-teams 182 s in one recent gate).
- **Comparison.** Pruning already frees about twice that ceiling (36 GiB), with no new contention.
- **If third-party sharing is wanted later.** A compiler cache (sccache-style) for registry crates gets the 17.7 GiB without a shared lock. That is a separate change.
