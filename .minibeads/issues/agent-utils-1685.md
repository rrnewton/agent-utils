---
title: 'sandbox-checkout-alias: bind an Eden slot own checkout at a fixed path inside its box'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-15T19:30:00+00:00
updated_at: 2026-11-15T19:30:00+00:00
---

# Description

## Goal
Bind each Eden slot's own checkout at one fixed absolute path inside its box, in both userns and root isolation, so tools that expect the checkout at a fixed location work in every lane.

## Evidence
The slot view binds the slot tree with a descriptor-based bind, which is also the only way the view can bind an Eden FUSE mount owned by another user namespace (a plain bind of it from inside the box does not work). The alias target cannot be a repository default naming one consumer's checkout location; it is a slot-view setting (`SlotView.checkout_alias`) chosen by the caller, with `wrkslots run --checkout-alias PATH` and `wrkslots shell-command --checkout-alias PATH` to set it. A follow-on in the agent launcher can later make the alias the default working directory for Eden slots.

## Fix
`slot_view` records the Eden checkout, its host backing store, and other slots' state directories on the view. With an alias set, `build_spec` adds a writable bind of the slot's own checkout at the alias target through the same pinned-descriptor bind the slot tree uses. The target must be an existing empty directory on the host (a one-time host-side creation) or a path below the box's fresh `/tmp`; a missing host directory is refused naming the `sudo mkdir` that creates it. The box never creates a directory on the host root.

## Acceptance
Through the alias in a scratch Eden lane: the checkout reads and writes, `eden info` resolves the same mount, a live build tool daemon and a file-watching query work against the alias path, and the extra daemon memory of a second lane over the first is measured and reported. A missing host mount point refuses with the creating command named. Unit coverage pins the bind, the refusal, and that non-Eden views are unchanged.
