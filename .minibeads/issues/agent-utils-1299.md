---
title: 'harness-layout-mode: declare harness and product repos so slots stop copying the harness'
status: open
priority: 0
issue_type: feature
created_at: 2026-11-11T14:48:05.720416712+00:00
updated_at: 2026-11-11T14:48:14.570049811+00:00
---

# Description

Owner design request (Slack 6230, 2026-11-11), relayed as five points; whether is settled, the design is ours. Quoted points: (1) Single-repo projects: setup's encouraged layout puts the primary checkout one level in, at <project>/<repo>/ beside <project>/worktrees/. Config and worktrees should NOT sit inside a checkout of the product repo (gitignored). (2) Recognize the development harness idiom explicitly: a parent harness repo pins the product repos as submodules and accumulates docs, experiments and scaffolding that do not belong in the product repos; config declares which repo is the harness and which are products. (3) Mechanistic discouragement: slot creation refuses a worktree of the harness repo unless a descriptively named flag is passed and records the stated reason; status and audit list such slots separately; optionally a shorter TTL. (4) Product slots use the shared submodule store by default in harness mode. (5) The setup wizard detects the harness shape (a repo whose submodules are the products) and offers this mode.

Evidence: in a consuming project 135 of 147 slots were full harness-repo worktrees (about 265 GiB of private submodule gitdirs), because spawn scripts used the harness repo as the slot repo and nothing stopped it. Each such slot copies every product submodule instead of checking out one product, so disk, clone time, and audit noise scale with the harness rather than the task.

Open question in (1), answered from the code at filing time: yes, the in-checkout layout is supported today -- init inside a product checkout with the worktrees directory under it is accepted, and create explicitly allows the selected source worktree to contain the managed root (the repo-=. case). Setup does steer away from it: the setup skill builds a harness root with the primary checkout one level in beside worktrees/ and gitignores both. Nothing in the slot tool itself names the in-checkout layout as discouraged, warns on it, or distinguishes harness from product repos, which is the gap points (2)-(5) close.

# Design

Add an explicit harness mode to the slot configuration: the config may declare one harness repo and one or more product repos (names as create already takes them). Absent the declaration, behaviour is exactly today's, in-checkout layout included.

In harness mode: (a) create refuses a slot whose repo set includes the harness repo unless a descriptively named opt-in flag is passed (spelling in the request, e.g. --include-harness-checkout-i-know-this-is-heavy) together with a stated reason, which is recorded in the slot record; status and audit list harness-checkout slots separately with that reason, and they may carry a shorter default TTL (coordinate with the TTL work). The refusal names the product alternative (create the slot from the product repo the task touches). (b) Product slots default to the shared submodule store (share_submodules) so product checkouts share one submodule object store instead of private copies; an explicit opt-out keeps today's per-slot copies. (c) create, status, and audit all read the declaration from the same config source; a repo named as both harness and product, or a product list naming an unconfigured repo, refuses at load.

Setup wizard: detect the harness shape (a repository whose submodules are the candidate products, with harness-only tracked content) and offer harness mode with the detected declaration pre-filled for owner confirmation; never enable it silently. For single-repo projects setup keeps encouraging the primary one level in beside worktrees/, and its text states plainly that config and worktrees inside a product checkout, though supported, are the layout to avoid (gitignored state inside the product tree, slot scans walking it). Docs (init help, quickstart, user guide, setup skill) name both layouts and which one setup steers to. No migration of existing slots: the declaration affects new creates and reporting only.

# Acceptance Criteria

- With a harness declaration configured, creating a slot that includes the harness repo refuses without the descriptive opt-in flag, naming the product-repo alternative; with the flag and a stated reason it succeeds, the reason is in the slot record, and status/audit list the slot separately as a harness checkout.
- A product-only slot in harness mode uses the shared submodule store by default (one object store across slots, verified by inode/store path in tests); opting out restores per-slot copies. Without a harness declaration, create/status/audit behave exactly as today, in-checkout layout included.
- A config naming a repo as both harness and product, or a product that is not configured, refuses at load naming the repo; no slot is created.
- Setup on a harness-shaped tree (submodules are the products) offers harness mode with the detected declaration and enables it only on owner confirmation; setup on a single product repo offers the one-level-in layout and states that in-checkout config/worktrees are supported but discouraged.
- Existing slots (harness checkouts included) keep working unchanged after the declaration is added: heartbeat, finish, remove, and audit verdicts are unaffected apart from the separate listing.

# Notes

Owner directive Slack 6230 (2026-11-11) via the coordinator; P0 for implementation by a fresh lane after the test-quarantine work it references. Issue-only here. The five points are quoted in Description; the in-checkout answer was read from init/create handling and the setup skill at filing time (supported today, steered away from by setup, unnamed in the tool). Related: shared submodule store and slot TTL work already tracked separately.
