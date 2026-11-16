---
title: 'vibe-talk-split-parts-not-regrouped: a bridge reply posted as three close thread parts stayed three rows'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- grouping
created_at: 2026-11-16T13:40:00+00:00
updated_at: 2026-11-16T13:40:00+00:00
---

# Description

Owner report, 2026-11-16 8:23 AM ET: "But I also didn't understand why vibe
talks grouping feature didn't recombine them. Since they were sent close in
time." "Them" is one long bridge reply posted as three Slack parts in one
thread, 1.4 s and 1.7 s apart, by the bridge app.

## Facts (read from the live cache, read-only)

The three parts (thread `1794832497.573659` in the owner's channel), oldest
first: `12:36:46.871089Z` (137 chars), `12:36:48.272819Z` (3995 chars,
+1401 ms), `12:36:50.007799Z` (2397 chars, +1735 ms). All three agree on
everything combining asks for: same thread, not roots, same author id (the
bridge app's account), bot, adjacent in the thread with no other channel
message between them, and not noise (the live noise-rules table is empty, so
only the default placeholder rule applies, which none of them matches).

## Root cause

The owner's **Combine messages** setting is off. The synced client settings
hold `vibe-talk.voice.combine = "0"`, written 2026-11-02 13:17 ET. The switch
is on-unless-off (absent means on), so a stored `0` turns combining off in
every view and on live arrival — the grouping code is doing what it is told.
No grouping-code defect was found: with combining on, the same three-part bot
post in a thread is one row when read in the thread, when read in All, and as
the parts arrive live.

The owner's remedy is one tap (Settings, Combine messages on); the rows
regroup at once, with no reload and no deploy. The stored setting is the
owner's and was not changed or overridden in code.

## Coverage added

Page tests in `vibe-talk/tests/js/page/rows.mjs` (`#1714`): three close parts
of one bridge reply in a thread are one row read in the thread and in All,
one row as they arrive live in the thread, and three rows with Combine
messages off that come together when it is turned on. Before this, the
split-post tests covered only top-level posts.
