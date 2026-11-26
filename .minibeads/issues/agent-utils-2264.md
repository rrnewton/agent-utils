---
title: 'vibe-talk-all-view-older-load-stuck: in the All view, Loading older shows but never loads, so summary citations point past the scrollback'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T17:40:00-05:00
updated_at: 2026-11-25T21:51:00-05:00
closed_at: 2026-11-25T21:51:00-05:00
---

# Description

Owner report (2026-11-25, about 5:25 PM ET): in the All view, the Discuss timeline went stale quickly. Its bullets cited messages older than the All view's current scrollback. Scrolling up showed "Loading older" but nothing older loaded. After a page reload, clicking one of those citations did reach the message, so the citation-jump path can walk back far enough and the scroll-up older walk cannot. Loading older messages from the vibe-talk server should be fast (low latency, high throughput).

A read-only look at the live server's log (timings only) found that every page "older" read in the owner's busiest channel that day returned status 200 with `served_from="cache-older-empty"`, and every one took about 1.5 s. So the server's cache had nothing older than what it had already served, waited a fixed time and answered empty. Since #1227 a page read never starts an upstream read itself, so an older walk past the cache's floor may have no way to trigger a backfill.

# Design

Diagnose first, on a local server only (never the live instance or its tokens):
- Reproduce: All view with a long history, scroll up past the loaded page repeatedly; also after a cache-first page, and after a citation jump and Back (compare with #2261's Back race, which also ends in an older-page load).
- Name the fixed wait behind the ~1.5 s empty answer and the code that serves `cache-older-empty`.
- Find why an empty older answer does not schedule a server-side backfill, and what the citation-jump path does differently that lets it reach older messages after a reload.
- Find what the page shows after an empty older answer (keeps "Loading older", gives up, retries) and whether the All view's merged older cursor advances.

Fix, within the quota rules: a cache-older-empty answer schedules a server-side backfill (paced, deduplicated, owner-quota aware; no upstream call from the page read itself, no per-page storms) and publishes what it finds by live event, as #1227 does for refresh. The page shows an honest state while a backfill is pending and fills in by live event, then lets the reader keep walking older.

# Acceptance Criteria

- The root cause is named with a reproduction command and trace on a local server.
- In the All view, scrolling up past the cache's floor loads older messages (from a server-scheduled backfill delivered by live event), or shows an honest pending or end-of-history state; it never sits on "Loading older" with nothing coming.
- No page read starts an upstream read; backfills are server-scheduled, paced and deduplicated (the #2251 cache-only guard test stays green).
- A page or server test that fails on main covers the stuck older walk, through the real server route the page calls.
- Evidence at phone 412x915 and desktop 1280x800, light and dark, with a BUILD.txt naming the commit.

# Notes

Owner request 374, part B (parts A and C, the Discuss pane split and the earlier-summary strikethrough, are separate lanes).

Root cause (2026-11-25, reproduced on a local server against a fake Slack):
- After any dropped cache offer, the copy refuses every step back whose claim was listed before the drop.
- Nothing listed those stretches again: a step back's page is offered as rows only, and a Slack All page that holds a thread is no listing of Main.
- So every step walked the chat service from its newest page. The walk lost the race against the 1.5 s step wait, and the step was answered empty at the same cursor. The walk's own answer was dropped, and nothing it read made the next step provable.
- Each repeat started another walk, with no claim to stop it.
- The page took each empty answer as a normal step and fired the next one on the next scroll event.
- A second defect sat on the same path: the page never drew a step back answered from the copy for Slack All. Every such answer is marked refresh_failed, and the page would not lower the view's floor for it.
- The citation the owner clicked after the reload made no older read at all: the reloaded page already held the message.

## Resolution

[opus 5.5] Landed on github/main as `f25f33aff8bdee310143b1cf33bc507da1677783`: the gated head `40146d30` (owner-priority gate pass, full selection) replayed onto `9786b3ab`, with an identical patch. It is the reviewed and accepted `daadf308`, rebased only. Deployed to the live instance at 9:36 PM ET on 2026-11-25; the post-deploy watch was clean (startup probe passed, no app errors or panics, no restarts, both services active).

- Server: one walk per channel view at a time, under the cache's backward claim. The first step waits the 1.5 s step wait for it. Every other step of the view, and every step while a failed walk backs the view off, is answered at once with `older_pending` and begins no upstream read. For Slack's Main and All the walk re-lists Main below the page: one history call per page, resuming where the view's last re-list stopped when no drop came since. The copy then proves the step and answers it. Elsewhere, the provider's own walk runs, and a late answer is kept for the next step. The walk reads under the `older-walk` reason at the page share of Slack's budget, below live work. Refusals retry on the #1715 schedule; failures back the view off. It publishes the live `older` event to its own step and to every step that waited. One `consumer="older_walk"` log line per walk.
- Page: "Fetching older messages from <service>…" while pending, with no further step on scroll. The `older` event takes the step again. A failed walk says so, with tap to retry. A step answered from the copy now lowers the view's floor, so its rows are drawn (the second defect).
- Contract: `TimelinePage.older_pending`; `LiveOlderEvent` (SSE `older`).
- Tests that fail on main: message_cache_serve (five steps under a held Slack make one Main read, then the step is served from the copy, with the event and the walk line; a failed walk sends ok=false and backs off; a step waiting on a partial answer's read hears the event; All walked back to its start, one walk at a time); the slack budget test (the walk stops at half the ceiling, live keeps its share); scrolling.mjs (pending with no scroll loop and a re-step on the event; failed and tap retry; a copy step's rows drawn).
- Acceptance, on a local real server against a fake Slack after a drop, at 412x915 and 1280x800, light and dark: scrolling past the copy's floor in All shows pending at about 1.6 s and 50 older rows at about 12.2 s (one walk, 2 older reads); the next step answers in 205-253 ms; the walk reaches the channel's start. Screenshot matrix from landed main (BUILD.txt f25f33af).
- Screenshot note (#1989): with the shared harness's documented command, scene 77-composer-keyboard-open failed twice at dark laptop-1280 when it ran right after scenes 73-76 in one run ("the composer holds the focus" not reached). It passed run alone, and in the gate's full desks walk on the same build, so the matrix was shot as two runs (73-76, then 77).
