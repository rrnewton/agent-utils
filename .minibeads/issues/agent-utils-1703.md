---
title: 'box-watchman-alias: make watchman work for a lane addressed through the checkout alias'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-16T00:35:00+00:00
updated_at: 2026-11-16T00:35:00+00:00
---

# Description

## Goal
A boxed Eden lane that works through its checkout alias must get working file watching, so a build tool that watches through watchman on a real (large) checkout does not degrade or fail when addressed only by the alias path.

## Evidence
Found while landing #1685 sandbox-checkout-alias and #1689 box-eden-read-visibility (4b2f91cb). On a scratch Eden lane, a real build binary built successfully through the alias and a file written through the alias was found by the host watchman daemon when addressed by the checkout's real path. But the host daemon, running outside the box's mount namespace, cannot resolve the box-local alias itself ("RootResolveError: realpath of the alias: No such file or directory"), and an in-box daemon could not be started on the measuring host at all: its site spawner starts it as a systemd service (the start is ignored inside the box's chroot-like environment) and a directly started daemon refuses to run at the session's nice value of 10. A trivially small build succeeds without watching; on a full checkout, where the build tool relies on watchman for file notification instead of crawling, this is expected to degrade or fail.

## Fix
Make one of these work, in this order of preference: map the alias to the real checkout path when a boxed command talks to the host watchman daemon (so the daemon resolves a path it can see); run a watchman daemon inside the box through a non-systemd spawner the box can start (with its state under the box's own tmpfs); or configure the build tool inside the box to address the checkout by its real path while sources stay addressed by the alias. Keep the alias itself a bind of the same mount, never a second checkout.

## Acceptance
On a scratch Eden lane addressed only through its checkout alias: a watchman query (watch plus find) returns the checkout's files and sees a file written through the alias, without addressing the checkout by its real path; and a watchman-using build through the alias completes with file notification active rather than a full crawl or a watch error.
