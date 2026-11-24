---
title: 'agentctl-watch-phantom-background-misses-live-child: the phantom-background notice must find every live background process before calling it phantom'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- watch
created_at: 2026-11-24T16:30:00-05:00
updated_at: 2026-11-24T16:30:00-05:00
---

# Description

The watcher's phantom-background check (#2045) reports a lane as "idle waiting on background work the screen counts (1 background shell), but no shell runs under its harness". On 2026-11-24 it said this about a Muse lane whose background shell was real and alive: a long-running await on a remote build job, still in progress. The notice suggests a restart, and a restart kills Muse's backgrounded tasks. Acting on this false positive would have thrown away the lane's in-flight wait.

So the process lookup that decides "no shell runs under its harness" misses at least one shape of live background process. Plausible causes: the shell or its child lives in a different cgroup or scope than the harness, it was re-parented (double fork or setsid) out of the harness process tree, or the lookup matches only certain command names.

# Design

- Find every process that belongs to the lane's background work by more than the process tree: the harness's cgroup subtree, the session or process group the harness's shells were started in, and the processes re-parented away from it. Count a background shell as live if any process in those sets is alive.
- When the evidence is ambiguous (the count says N shells, the lookup finds fewer, and the harness's cgroup can't be read), say "unverified" instead of "phantom", and never suggest a restart from an unverified check.
- The notice names the process ids it did find, so an operator can check before acting.

# Acceptance Criteria

- Test: a lane whose background shell has a long-running child that was re-parented out of the harness tree (setsid or double fork) but still sits in the harness cgroup isn't reported as phantom.
- Test: a lane whose count says 1 shell, where truly nothing is alive in the cgroup, session or tree, is still reported as phantom, so the #2045 behaviour is kept.
- Test: an unreadable cgroup gives "unverified", with no restart suggestion.

# Notes

Reported by a consumer team's lead on 2026-11-24, who checked the live process before acting. Related: #2045 (a finished background send stays counted as running in Muse).
