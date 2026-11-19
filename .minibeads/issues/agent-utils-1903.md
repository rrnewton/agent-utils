---
title: 'vibe-talk-thread-reader-follow-disabled: thread-reader-follow-browser disabled until robust on a hot host'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-19T01:20:00-05:00
updated_at: 2026-11-19T01:20:00-05:00
---

# Description

[watermelon-full-preview] `vibe-talk.check.thread-reader-follow-browser` is disabled: the check script exits 0 at the top with a DISABLED line, and this issue is its tracking note. It failed repeatedly in gates on 2026-11-18 (three times in the day's gate ledger, including after #1866 vibe-talk-read-follow-clears-reader-strip hardened its fixture with whole-block speech and one request per connection), and on 2026-11-19 mu-nestedlink reproduced it SOLO on pristine main 8d190338: a `#jump-reading` click timeout. It then failed the gates of #1898 and #1901 within fifteen minutes, blocking owner-UI landings, so it leaves gating now under owner rule 346 (a check that is not robust on a hot host does not gate).

# Design

- The script stays runnable by hand (`make -C vibe-talk thread-reader-follow-browser`); only its gate verdict is suspended, by the early return in `main()`.
- The follow-up in mu-readerdrill's #1880/#1872 owns the underlying failure (the Reading-chip jump click and the follow walk's timing on a loaded host).
- When the step returns as a DAG step again (the dag.d fragment after #1891, replacing the script-level early return with the step's own disabled-optin removal), it comes back behaviour-only: scroll position and chip behaviour, no wall-clock verdicts.

# Acceptance Criteria

- The #jump-reading click timeout is reproduced and understood (solo, on main), not just seen under load.
- The walk passes solo repeatedly and 10/10 with CPU burners pinned to its own cpuset, per owner rule 346.
- The early return (and, later, any disabled-optin label) is removed in the same change that proves the above, and gates run it green.

# Notes

Related: #1856 vibe-talk-read-aloud-follow-not-yanked-up (the regression the walk guards), #1866 vibe-talk-read-follow-clears-reader-strip (today's hardening, which the failures postdate), #1872/#1880 (mu-readerdrill follow-up). Survey context (2026-11-19): the step's median gate time was about 24 s with 3 load failures that day; the solo failure is what moved it from "keep and recheck" to disabled.
