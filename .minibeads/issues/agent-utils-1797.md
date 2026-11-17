---
title: 'vibe-talk-search-bucket-toggles: search hits bucketed by scope behind toggle chips under the search bar'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- search
created_at: 2026-11-17T14:10:00.000000000-05:00
updated_at: 2026-11-17T14:10:00.000000000-05:00
---

# Description

[watermelon-full-preview] Owner, 2026-11-17 (owner threads 144-146, refining #1786 vibe-talk-search-scope-server-hits, final wording in 145/146): the one outside-scope chip becomes a row of count chips under the search bar, one per bucket, each an independent ON/OFF toggle styled like the Documents view filters:

- Up to FOUR chips, each shown only when its count > 0: "N in this thread", "N in main", "N in other threads", "N in Agent Transcripts" (the last only once #1608 exists; build the row so mu-transcripts can plug that bucket in).
- Hits from every enabled bucket interleave chronologically: this-thread hits as normal cards, the others in the quoted style ("in thread <slug>" headers).
- No double counting: a main message that is also a thread root counts ONCE, in "main", never in "other threads".
- One search view, identical from every screen; only the default-on chips differ (owner 146):

| Searching from | Chips shown (count > 0 only) | ON by default |
| A thread | this thread / main / other threads / transcripts | this thread |
| Main | main / threads / transcripts | main |
| All | main / threads / transcripts | main + threads |
| Agent Transcripts view | main / threads / transcripts | transcripts |

Outside a thread view the bucket is "threads" (not "other threads").

# Acceptance Criteria

- The chip row stands under the search bar in its own row (pushing content down, never floating over cards or controls), with one toggle chip per bucket whose fresh-hit count is > 0; chips read "N in this thread" / "N in main" / "N in other threads" (thread view) or "N in threads" (elsewhere), styled like the Documents view filters, with pressed state legible at a glance.
- Toggling a bucket interleaves or removes its hits chronologically in the timeline: the home bucket (this thread in a thread view, main elsewhere) as normal cards, other enabled buckets quoted with "in thread <slug>" headers where threaded; showing a bucket brings its first newly shown hit into view through the scroll owner.
- A thread root counts once, in "main"; a reply counts in its thread's bucket; no hit is counted or drawn twice.
- Defaults follow the owner-146 table for the screen the search starts from; the row is built so a fourth "Agent Transcripts" bucket can be plugged in without restructuring (no chip renders until such hits exist).
- Typing still filters instantly (#1762): bucket counts and placement update only when the deferred server answer lands, never per keystroke; the search count ("N of M loaded") still speaks only for loaded messages.
- Page tests at phone (412x915) and desktop (1280x800) cover the buckets, defaults and toggles, in light and dark, keyboard open where layout is involved, Discuss half-pane open in at least the phone shots.
- Screenshot matrix per the standing acceptance PLUS one set per starting screen (thread, Main, All; Transcripts with a fixture until #1608) and the toggle combinations (only this-thread; this-thread + main; main + other threads; all on; this-thread off), reviewed image by image, folder path and one line per image in the report to coordb.

# Notes
