---
title: 'vibe-talk-record-completeness: the cached record withholds owner thread replies as in doubt'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-14T00:00:00.000000000+00:00
updated_at: 2026-11-14T00:00:00.000000000+00:00
---

# Description

The owner's record of their own prompts must be complete. `vibe-talk search "collapsable sections" --channel C0CC755QYSK` reported "No hits ... 3 withheld (thread in doubt)" for the owner's thread reply ts 1794503204.193669 (11-12 12:06 PM ET, thread root 1794502902.037749), and `recap --hours 60` missed it too. Thirteen owner thread replies from 11-12 (5:59 AM, 6:31 AM, 12:06 PM, 12:09 PM, 2:24, 2:26, 2:27, 2:31, 2:46, 2:48, 3:11, 3:13, 3:14 PM ET) are the acceptance cases: the cache must hold all 13, none "in doubt". Audit of a read-only cache copy shows test case 1 is present with full text; it is withheld at search time. One dropped cache offer marks the whole channel suspect (`channel.dropped_ms`), and search then withholds every hit not re-observed since, including thread replies no relist ever re-reads.

# Design

Keep the serving rules for pages (a dropped offer may have been an edit or deletion, so a page must not vouch for stale rows), but make the record surfaces complete: a held row with whole text must not be withheld from search solely because a later channel-wide drop postdates its observation. Withholding stays for a thread scope whose access is genuinely in doubt (blocked) and for rows held only as push snippets. The live instance must fill any remaining gap after deploy without manual surgery (backfill on start or on relist) and without raising its Slack call rate beyond the cache design's allowance.

# Acceptance Criteria

- All 13 acceptance replies are held by the cache and returned by search, none counted withheld for a channel-wide drop.
- Hermetic fixture-store regression tests reproduce test case 1 (injected clock, no wall clock, no network): a matching whole-text reply observed before a channel drop is still found.
- A 7-day owner completeness audit (cache copy vs one bounded paced upstream read) lists every missing or withheld owner message with ts, channel, thread root and why; the report lands in the harness research record with a changelog bullet.
- The wrkviz cross-check (scratch archive only) answers whether owner prompts to coordinators are told apart from agent-to-agent prompts, with evidence and gaps; no wrkviz change in this lane.

# Notes

Owner 062, 11-14 10:56 AM ET: a record or CLI that misses prompts is a top-priority bug. Sibling lane mu-prompts owns #1528 (recap flat walk) and #1529 (`vibe-talk prompts --hours K`, cache-only); this lane owns what makes the cache itself incomplete or "in doubt", plus the audit. No consumer names, host names or local paths are recorded here.
