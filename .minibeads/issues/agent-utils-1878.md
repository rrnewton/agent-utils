---
title: 'wrkslots-nested-module-name-vs-path: populated check conflates submodule name and path'
status: closed
priority: 1
issue_type: bug
assignee: unassigned
labels:
- wrkslots
created_at: 2026-11-19T05:00:00.000000000+00:00
updated_at: 2026-11-19T05:00:00.000000000+00:00
---

# Description

An older data-loss bug found by the #1785 round-3 review (not introduced by it): in `py/wrkslots/cli.py`, `_nested_module_repository_checkouts` builds `relative` from the module NAME (`modules/<name>` under the checkout's Git directory), and `_submodule_worktree_populated(checkout_path / relative)` treats it as the worktree PATH. For a submodule declared with `--name y` at path x, deinitialized after gaining a private commit (its Git directory stays at `.git/worktrees/<checkout>/modules/y`), while another submodule sits at path y, remove succeeds, the private commit is in no bundle, and its Git directory is deleted. Control: with the second submodule at path z the commit is bundled; `git mv` creates the same name/path mismatch. Medium, same change: in `rs/wrkslotsd/src/remove_write.rs`, a `-` line from `git submodule status` is treated as "never initialized, nothing to salvage", but a deinitialized submodule shows `-` while `modules/<name>` still holds private history; the Rust edition has no counterpart to `_nested_module_repository_checkouts`, so fail closed (refuse when a `modules/<name>` Git directory exists for a `-` entry).

# Design

Skip the nested-module candidate only when the populated worktree's Git directory resolves to the module Git directory being examined, or read the module's `core.worktree` rather than assuming name == path. Mirror the fail-closed behaviour in the Rust gate. Deterministic tests: name-vs-path mismatch with a private commit in the deinitialized submodule (bundled or refused, never silently deleted), the z control, and the Rust `-` refusal.

# Acceptance Criteria

The y/x mismatch case keeps or refuses on the private commit in both editions; the z control still bundles; no other remove behaviour changes.

# Notes

## Resolution

Landed as 3d3eb9d4 ('wrkslots: judge nested module repositories by their own worktree'). The Python scan resolves the worktree a module itself records (`gitdir` file, then `core.worktree`), falling back to the declaring superprojects' `.gitmodules` name-to-path mapping chained per nesting level — the only record `git submodule deinit` leaves behind — names the salvage candidate by that declared worktree path, and refuses when the name-spelled path is another repository's populated worktree and nothing records this module's own. The placeholder directory for reading an unpopulated module is made only inside the current checkout, never at the recorded path, so a post-fence recheck cannot resurrect the slot being removed. The Rust remove gate fails closed when a `-` submodule-status line retains a module repository under `modules/`, handing the slot to the Python edition. Pre-fix reproduction: y/x mismatch lost the private commit (receipts only product, product/y); z control bundled. Post-fix: y/x and z both bundle, Rust refuses deinitialized stores.
