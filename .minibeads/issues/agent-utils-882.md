---
title: 'wrkslots-insteadof-diagnosis: a fetch through a rewritten URL that fails names the rewrite and the cause'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T10:57:10.530106531+00:00
updated_at: 2026-11-05T10:57:10.530106531+00:00
---

# Description

A global `url.<base>.insteadOf` rewrite on a host redirected a submodule's upstream URL to a module directory inside a consumer project's own `.git/modules/...`. That directory was a stale mirror:

(a) It lacked a commit (`d7901952`) that a new gitlink needed.
(b) Its `core.worktree`, and that of five sibling module dirs, pointed at paths that a first-party split had removed. So every serve from it died in `git pack-objects` with "cannot chdir".

The post-provision hook failed with rc 128, and every new slot for that project was refused until an operator worked around it by hand. The rewrites live in the user's global Git config, which agent-utils does not own. The defect here is wrkslots' handling: the failure surfaced as an opaque rc 128 on every create.

This is related to the insteadOf pitfall noted in #871.

# Design

- When a fetch or clone during create/provision fails, detect whether the URL was rewritten (`git config --get-urlmatch` / `git ls-remote --get-url`). If so, the error names:
  - the original URL;
  - the rewrite rule and its config file;
  - the target;
  - the specific cause where it can be found: the missing object, or a `core.worktree` that does not exist.
- Optionally, with a recorded NOTE, retry once against the original upstream URL with rewrites disabled (`GIT_CONFIG_GLOBAL=/dev/null`, or `-c url.<base>.insteadOf=`) before failing the create. Gate this behind a config key if the retry has a cost.
- If wrkslots or setup owns any mirror refresh, refresh the mirror on demand when an object is missing.

# Acceptance Criteria

- A test with a global insteadOf rewrite (set in a temporary HOME / GIT_CONFIG_GLOBAL) pointing at a mirror that lacks the needed commit: create fails with the named rewrite and the missing object, or succeeds through the upstream retry when that is enabled.
- A test where the mirror's `core.worktree` is dead: the error names it.

# Notes

Reported by the lead on 2026-11-05 at 06:00 AM ET, from a consumer team (P2). The consumer team has already worked around it. Assigned to mu-salvage after #871.

Moved from mu-salvage to the new Muse ultra wrkslots lane mu-wrkslots on 2026-11-05 at 5:50 PM ET (the owner lifted the Muse lane cap; the lead asked for a dedicated wrkslots lane).
