---
title: 'agent-utils-node-outside-owner-checkout: builds get a node outside any off-limits path'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- build
assignee: mu-maskpaths
created_at: 2026-11-23T13:35:39-05:00
updated_at: 2026-11-23T13:35:39-05:00
---

# Description

The vibe-talk page build and the host deploy helpers need node 18 or later, but the hosts' system node is 16. Today they borrow a node binary from an owner-declared off-limits path, so a build that respects the mask in #2163 agentctl-profile-masked-paths would lose its node and fail, while a build that keeps working must enter the off-limits path. Builds need a node source that lives outside any off-limits path.

The real off-limits path is host-local configuration only. It never appears in this repository; this issue describes it generically as an owner-declared off-limits path, and the chosen node location is the owner's choice, recorded outside this repository.

# Design

Give the vibe-talk page build and the deploy helpers a node source outside any owner-declared off-limits path. The owner chooses which source that is (for example a toolchain installed in an allowed location); the build resolves node from that allowed source and from the system paths, never from inside an off-limits path. When no node 18 or later is found outside every off-limits path, the build fails loudly at startup, naming the requirement (node 18 or later, outside any off-limits path) and how to provide one, rather than silently falling back to the borrowed binary or to the system node 16. This issue is the prerequisite for enabling the mask in #2163 agentctl-profile-masked-paths on any real profile: masking reaches no real profile until this issue is live.

# Acceptance Criteria

- The vibe-talk page build runs to completion using a node 18 or later resolved from outside any owner-declared off-limits path, with the off-limits path masked or otherwise unreachable.
- A deploy-style build (the host deploy helpers' build step) likewise runs using only a node from outside any off-limits path.
- With no suitable node available outside every off-limits path, the build fails immediately with a message naming the node 18-or-later requirement and the off-limits constraint; it does not borrow a node from inside an off-limits path and does not silently use node 16.
- No real path, host name or checkout name appears in code, tests, docs or commit messages; the allowed node source is owner-chosen configuration outside this repository.

# Notes

[watermelon-full-preview] mu-maskpaths. Number pre-assigned by the coordinator. Filed alongside #2163 agentctl-profile-masked-paths, which is blocked by this issue: the mask must not be enabled on any real profile until builds no longer need to enter an off-limits path for node. Owner chooses the allowed node source; this issue builds the resolution and the loud failure, not the choice itself.
