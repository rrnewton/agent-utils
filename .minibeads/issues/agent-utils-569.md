---
title: 'vibe-talk-test-debuginfo: line tables only in debug builds, and one integration-test binary'
status: closed
priority: 2
issue_type: chore
created_at: 2026-10-27T01:58:10.934205582+00:00
updated_at: 2026-10-27T01:58:10.936814114+00:00
closed_at: 2026-10-27T01:58:10.936813994+00:00
---

# Description

[opus 5.5] A dev-profile build of vibe-talk's tests filled about 30 GB per worktree: 91 integration-test binaries of about 330 MB each with full debuginfo, the main cause of disk bursts on the shared validation host. Set [profile.dev] and [profile.test] debug = line-tables-only with dependencies at debug = false, and merge the integration tests into one binary (keeping the two that need a process of their own). Measure target size before and after.
