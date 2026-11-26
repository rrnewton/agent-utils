---
title: 'vibe-talk-client-debug-mode: an owner switch that records client freeze attribution and uploads it with the scroll traces'
status: closed
priority: 0
issue_type: feature
assignee: cl-debugmode
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T03:40:00-05:00
updated_at: 2026-11-26T11:44:00-05:00
closed_at: 2026-11-26T11:44:00-05:00
---

# Description

Owner request 379 (2026-11-26 3:31 AM ET), verbatim: "Very laggy client right now. Can you see any of that server side? Can I have a debug mode with some local client logging or sampling that gets uploaded along w the scroll traces. My [busy project] channel updated very slowly. Was laggy while refreshing. Screen jumped a lot while I was trying to type a message. And the typing itself was laggy and got frozen some."

The page already posts freeze reports (`#2066 vibe-talk-page-freeze`: kind longtask/freeze, the reader's last act, view, layout) and scroll traces. In the night's 178 freeze reports the script field was empty in every one, so nothing says what held the main thread. This issue adds the instrument; the standing perf lane finds and fixes the cause of the night's freezes separately (`#2277`).

# Design

A client debug mode (`web/src/view/shell/clientdebug.ts`, server `src/client_debug.rs`): one owner switch, off by default, kept on the device (`vibe-talk.voice.client-debug`) and shown in Settings > Debug as "Client debug mode"; `?client-debug=1` turns it on for one load. Liaison-reviewed design (2026-11-26), with the liaison's changes folded in.

What it records while on, all on the `performance.now` clock of the existing records:

- Named spans around the page's own hot paths, wrapped at the composition root's call sites (`key`, `draft-typed`, `composer-fit`, `search-fast`, `search`, `search-settled`, `rows`, `rows-now`, `timeline-page`, `live-arrival`, `timeline-delta`, `resume-catch-up`, `reply-arrows`, `scroll-hold`, `scroll-reader`, `scroll-restore`, `scroll-follow`), each with its layout reads (`getBoundingClientRect`/`getClientRects`, counted only while on) and, for a span of 16 ms or more, the page's element count at its end. Also `performance.measure("vt:<name>")` for DevTools, cleared by name.
- Event Timing entries (`evt:<type>` event to paint, `evt-run:<type>` its handlers) and long-animation-frame detail (blocking, render, style-and-layout ms, the four longest scripts: function, file and position, invoker type and invoker) where the browser delivers them.
- Main-thread samples from the JS Self-Profiling API where allowed: the server sends `Document-Policy: js-profiling` on the shell only when the request carries the switch's cookie `vibe-talk-client-debug=1` or the URL flag (a separate ETag for that variant, so a 304 never keeps or withholds the policy). A stall's samples are summarised to its top six self and inclusive frames; raw samples never leave the device. The sample interval (`DEBUG_SAMPLE_MS`) is the measured one that keeps the CPU overhead under 5%.

Per freeze event (`FreezeEvent.debug`): the spans inside the stall (name, count, ms, reads, nodes), the spans OPEN at its start, the last eight span ends before it, the stall's UNATTRIBUTED ms (no page span covered them), plus the frame detail and profile where available; detail on the ten longest stalls of a report only. Per report (`FreezeReport.debug`, `TraceReport.debug`): capabilities (`loaf`, `longtask`, `event`, `profiler`, `policy`, `sample_ms`), span totals since the last posted summary (top 16), long-frame counts over 50 and 100 ms and the 20 slowest frames. Besides those, at most one summary-only freeze report a minute, and only after something slow (a frame of 100 ms or a span of 50 ms).

Privacy: timings and code names only. The page strips URL queries, fragments and content hashes, drops element ids that are not the page's own, and redacts any word that looks like an id; the server refuses any name outside the code-name alphabet (no `?`, `=`, `&`, `@`, quotes or separators) or with an id-like word (six or more digits in a row, sixteen or more hex digits, an upper-case word of eight or more characters with a digit: Slack-style ids), trailing `:line:column` positions exempt. A report the server refuses for its detail is re-sent once without it, so the freeze ring never wedges. The page never reads the user agent (a page ground rule); the server logs the browser class (family and major version only) from the request's own `User-Agent`.

Server: the existing routes take the new optional fields (`deny_unknown_fields` kept; freeze body cap 160 KiB, scroll-trace cap 272 KiB, each proven to fit a report at every cap) and log `page_freeze_debug i=N ...` under each event and `client_debug_summary route=... ua=...` lines. Cost with the switch off: no observer, no profiler, no wrapped method, no timer, no cookie, no header; a span is one test and the call.

# Acceptance Criteria

- Switch off: no extra observers registered, no extra marks, no profiler, no extra upload fields (page test).
- Switch on: long-frame attribution, named spans and (where available) profiler top frames are recorded, capped, and uploaded with the freeze reports and scroll traces (page tests at phone and desktop sizes).
- A real-server test: the routes accept the new fields, reject oversize or malformed ones, and log them; contract checks updated.
- Exercised on a local test instance through the real server, with the screenshot matrix for the settings switch.

# Notes

Owner-UI fast-track. Owning lane: cl-debugmode. Related: `#2066 vibe-talk-page-freeze` (the freeze watch this extends), `#2091 vibe-talk-shrink-anchor-policy` (its live invariant debug mode uses the same freeze channel), `#2277` (the night's freeze root cause, standing perf lane).

## Resolution

Landed 241aaf58 (replay of gated 6d7627f4; owner-tier gate pass, 226 steps, full selection): the client debug mode switch in Settings > Debug (and `?client-debug=1` for one load), named spans with layout reads, element count and S/D sizes, per-stall open spans, recent span ends and unattributed ms, long-animation-frame and Event Timing detail where delivered, and JS Self-Profiling at 20 ms (measured +2.7% to +3.1% renderer CPU on the phone walk; 10 ms cost +6% to +7.5%). The shell carries `Document-Policy: js-profiling` only for the switch's cookie or the flag, under its own ETag. The server takes the optional `debug` fields on the freeze-report and scroll-trace routes, refuses names outside the code-name alphabet or with id-like words, and logs `page_freeze_debug` and `client_debug_summary` lines with the browser class taken from the request's own User-Agent (the page never reads it, by an existing page rule).

Follow-up landed 6a40d891 (replay of gated c8f26f0d): spans inside the row redraw (the dress loop with rows counted, the per-row label measure as one aggregated record), the read landing (fold, projection, draw) and the scroll handler chain, plus `s_src` on S. Its full gate's only red was `vibe-talk.check.read-modes-browser`, the pre-existing `#2293` flake (it fails the same way on unmodified main); the exclusive rerun of that step passed, and the waiver was named in the landing notice. An earlier follow-up gate red on `vibe-talk.check.reload-place-browser` was cleared as load (5/5 alone on the tree, 5/5 on main, 14 overlapping steps in the gate's CPU box). The follow-up also greys the label picker's find-or-add field while offline (review nit).

S finding (`#2282`): on a threaded provider (Slack) reads land in the channel store, so S counts the store (`s_src=canon`); on an unthreaded provider the page draws its page reads without the store, so S counts the message rows drawn (`s_src=drawn`). Both were exercised on local fixtures (22 canon, 15 drawn).

To read the attribution once the owner turns the mode on: grep the server log for `page_freeze_debug` and `client_debug_summary`.
