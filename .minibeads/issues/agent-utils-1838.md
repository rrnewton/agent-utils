---
title: 'vibe-talk-reader-continues-into-thread: read-aloud continues into the thread and back out'
status: closed
priority: 0
issue_type: feature
assignee: mu-readerdrill
labels:
- vibe-talk
- ui
created_at: 2026-11-18T07:50:00-05:00
updated_at: 2026-11-18T14:35:00-05:00
closed_at: 2026-11-18T14:35:00-05:00
---

# Description

[watermelon-full-preview] Owner request, fast-tracked (owner thread 170, 2026-11-18), verbatim: "This is a very specific scenario. I was playing a message in read mode and I clicked the thread button at the top of the message to jump into the thread to see the other messages around it. We already talked about keeping the message at the same place on the screen when we jump into the thread. But if we were reading and then in the middle of reading a specific chunk, we should also restore the reader and continue reading. It's okay if we reread the smallest chunk if that's necessary."

Today a view switch stops the reader outright: `changeChannelView` in `vibe-talk/web/src/view/channel/channelviews.ts` calls `stopReading()` and clears reading mode on every switch, so tapping a playing message's Thread button silences it and the reader must find their place and start again. The drill-in anchor of #1818 vibe-talk-thread-drill-in-keeps-anchor (landed 6c0f9843) keeps the message's screen place; nothing keeps its reading place.

# Design

When Read mode is playing a message (server voice or device voice) and the reader drills into the thread containing it — the message's Thread button, or any other entry into that thread — the view switch captures the read before stopping it: the message, its parts, the smallest unit being read (the prepared block for the server voice, the device voice's chunk), and whether it was held. After the thread view opens and the #1818 anchor is restored, the read resumes on the same message at that unit (rereading that one unit is fine) and continues through the thread's following messages exactly as Read normally would in that view, autoplay included. Pace and voice settings are untouched. The same rule covers going back out of the thread while reading.

Fail safe: if the message is not in the view switched to, the read stops cleanly and the status line says so in place; no read starts on a message the new view does not show.

# Acceptance Criteria

- A deterministic browser test (condition waits only) starts Read on a message, taps Thread mid-message, and asserts the reader is still playing and resumes at the chunk it was on (or the one before), then continues to the thread's next message.
- Going back out of the thread while reading resumes by the same rule; a switch to a view without the message stops cleanly and says so on the status line.
- The screenshot matrix (light and dark, keyboard open really shrunk and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the reader playing after the jump with the highlighted chunk visible) is captured on a test instance and reviewed before announcement.

# Notes

Owner-priority gate: deployed on landing. Lane mu-readerdrill on branch mu-readerdrill/1838. Builds on #1818 vibe-talk-thread-drill-in-keeps-anchor.

## Resolution

[watermelon-full-preview] Landed on github/main as 7245b572371eb25dbfa5ab3db10c2bbc77f4dbfc (gated head 3c13ea5c, owner gate validate: OK, ledger head_sha 3c13ea5ce7a46cdf87ea5a5feaaccfc2b93ac238; verified-tree route on replay tree 42aae595). A view switch into or out of a thread captures the playing read (message, parts, the prepared block or device chunk it is on, held or not) and resumes it after the new view's entry is paid, at that smallest unit; pace and voice are untouched, autoplay continues in the new view, and the resumed follow re-places nothing while a composer is held (#1839). A switch to a view without the message stops cleanly, turns reading mode off, and says so on the status line. Chromium walks at 412x915 and 1280x800 assert the resume chunk, the anchor place, the next reply, Back, and the fail-safe (the base fails at the resume); the screenshot matrix (phone and desktop, light and dark, keyboard really shrunk and closed, Discuss open on the phone) was reviewed by the lane.
