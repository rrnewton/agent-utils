---
title: 'channel-search-hash: find a channel by name in any case, with or without its leading ''#'', and answer searches while typing at once'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T13:03:37.344908907+00:00
updated_at: 2026-10-19T13:33:54.180823565+00:00
closed_at: 2026-10-19T13:33:54.180823194+00:00
---

# Description

[opus 5.5 coordinator / claude worker] Owner, 2026-10-19: searching for a Slack channel by part of its name did not find it, and suspected case or the leading '#'; should the '#' be normalised away if every Slack channel has one?

Checked: GET /api/v1/channel-directory already matched a Slack channel such as `#team-builds-014` for `builds`, `Builds`, `team-builds-014` and `#team-builds-014`, but each search took about 6 s, because Slack's listing of the app's channels takes 6 to 11 s and was read again for every search. The bar's channel picker is a native <select> whose Slack options read `#team-builds-014`, so typing `builds` (the browser's type-ahead) never reaches it. That is the likely failure.

Wanted: finding a channel by name works everywhere, in any case, with or without the leading '#':
- The channel pickers (the bar's, which Ctrl+K opens, and Settings') find `#team-builds-014` when you type `team`. A native picker's type-ahead reads an option's text from its first letter, so the pickers list the channel without its leading '#' (the service's header, or Settings' Service row, already says it is a Slack channel); everywhere else the name keeps its '#'.
- The channel browser's search treats a leading '#' as optional and matches in any case, for every service (Slack, a Discord-protocol bridge, the fake).
- The voice agent's channel argument accepts a name with or without the '#'.
- Searching the browser feels responsive: while a search is on its way the page says so and for how long, and each service's listing is kept on the server for a short time (two minutes, per service, forgotten when a channel is added or removed), so further searches while typing answer at once.

Tests: server tests per service for the optional '#' and case, the kept listing (one upstream listing for several searches, read again after the time or an add), with negative controls; page tests; a Chromium check that types a name without its '#' into the picker and into the browser's search.
