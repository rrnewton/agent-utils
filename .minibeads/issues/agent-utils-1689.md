---
title: 'box-eden-read-visibility: an Eden lane sees other checkouts and its own eden info fails'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-15T19:30:00+00:00
updated_at: 2026-11-15T19:30:00+00:00
---

# Description

## Goal
In a userns slot box, an Eden lane must see only its own checkout and its own per-slot Eden state, and `eden info` for its own checkout must work.

## Evidence
A boxed-lane probe rerun found: `eden list` through the shared host daemon config directory listed every other host checkout, and the host backing store's `.hg/requires` was readable through the read-only $HOME bind (writes were refused, so write isolation held but read visibility did not). In the same box `eden info` failed with exit 77, "The shared directory / is not owned by you or root". Tracing the failing check inside the box showed Sapling walks the ancestors of a checkout's shared store to `/`, requiring each to be owned by the user or by root; inside a user namespace a host root-owned directory appears as the overflow uid, so the walk always fails at `/`, and at any other root-owned ancestor such as `/home`.

## Fix
For an Eden slot's view: mask the shared host daemon config directories under $HOME, every other slot's `.eden-state`, and the checkout's host backing store. The backing mask leaves a bare shared-directory skeleton (an empty `.hg/store` and an empty `requires` on a per-launch tmpfs) so the lane's own `sl root` still works while no host store file can be read. Re-root the namespace on a tmpfs owned by the boxed user (clone each top-level entry of the old root by descriptor, pivot, re-attach, and detach the old root once the pinned binds are made) and cover any remaining root-owned ancestors of the checkout and backing the same way.

## Acceptance
Inside a userns box on a scratch Eden slot: `eden info` for the lane's own checkout returns 0 (was 77); `eden list` with the lane's own config dir lists only its checkout and the shared host config dir lists nothing; the host backing store's `.hg/requires` cannot be read and other slots' `.eden-state` show as empty; the lane's own `.eden-state`, checkout reads/writes, and `sl root` still work. The same holds in root isolation.
