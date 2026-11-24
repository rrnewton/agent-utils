---
title: 'vibe-talk-browser-load-known-red'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-24T06:00:00-05:00
updated_at: 2026-11-24T06:00:00-05:00
---

# Description

Several vibe-talk browser steps fail often under host load (browser closed, waits expiring) while passing on quieter runs: the screenshots family, the Discuss family, and the search/read timing family. Each gets a separate entry in validation/known-red.toml under this tracking issue, with a shared 30-day expiry, so gates whose only reds are those steps can be waived at once while the root-cause work proceeds.

Entries (seeded 2026-11-24, all expiring 2026-12-24): vibe-talk.shots.screenshots-desks, vibe-talk.shots.screenshots-phones, vibe-talk.shots.screenshots-wide, vibe-talk.check.discuss-browser, vibe-talk.check.discuss-dock-browser, vibe-talk.check.discuss-strike-live-browser, vibe-talk.check.discuss-readjump-browser, vibe-talk.check.search-unmatched-browser, vibe-talk.check.read-pause-reply-browser, vibe-talk.check.search-typing-fast-browser.

thread-pin-picker is deliberately NOT listed: its frequent red was a real date bug, fixed in #2158, and reds there are real now.

# Acceptance Criteria

- Every interim entry is a separate known-red.toml entry marked #2175 with the shared expiry; an entry drops off when its step is proven fixed or hardened, and the whole set is renewed only under a live tracking issue.

# Notes

Filed by the coordinator from the browser-red plan (mu-clientperf, 2026-11-24); the owning lane refines this file as it works.
