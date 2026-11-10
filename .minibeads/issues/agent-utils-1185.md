---
title: 'chip-height-snap: resting inside the slack is snapped to the foot as chips resize'
status: closed
priority: 0
issue_type: bug
assignee: mu-scroll
created_at: 2026-11-10T03:08:53.221144610+00:00
updated_at: 2026-11-10T07:53:41.133902651+00:00
closed_at: 2026-11-10T07:53:41.047796049+00:00
---

# Description

Owner report (reply 294, 2026-11-09 9:48 PM ET) on the build with part 1 and 2a of #1144 scrollback-typing-jump live: "I refreshed but I'm still getting little jumps upward (spontaneous small scroll back)".

Leading mechanism (code-read, to verify with the scroll trace from #1144 part 2b): two legacy ResizeObservers in the page (the scroll-tools chips row and the read-aloud strip) write the list straight to its end whenever a re-derived at-the-bottom test reads true as they resize. A reader resting a few pixels above the foot - inside the 24 px slack, so the test still reads at-the-bottom - is snapped those few pixels to the foot every time a chip changes height (the Newest count changing, the draft chip coming and going, the read strip wrapping). Content jumps up a little, spontaneously, again and again.

Rule: following the foot is one explicit state (the foot keeper of #1144 part 1): on only when the reader is at the bottom by their own scroll or a landing, off as soon as they scroll up, never switched on by a programmatic scroll. These observers must ask that state, not re-derive at-the-bottom at resize time.

Follow-up to #1144 scrollback-typing-jump; the trace shipped there will show this writer's stack if the ranking is right.

# Design

Page-only, in the legacy script and the list keepers:

- Expose the foot keeper's follow state (Main's keeper in view/channel/listkeepers.ts) as a question the page can ask.
- The two legacy pinned observers (scroll-tools and read-steer in web/voice.js) ask that state - held through a holder set once the keepers mount, as the keyboard room is - instead of followsNewest at resize time. Their other exclusions stay (a thread-entry landing, a filter hold, a quick-reply hold).
- Nothing else changes: readers genuinely at the foot are still kept on it as chips and the strip resize; readers a few pixels up are left exactly where they are.

Tests: a browser case in the typing-scroll walk (desktop): resting a few pixels above the foot inside the slack, a chip height change (a draft beginning) moves nothing; and the existing unit pins for the follow state.

# Acceptance Criteria

- Resting a few px above the foot (inside the old 24 px slack) with the composer focused, a chip height change moves the list by 0 px; the same at the foot itself still follows to the foot.
- The scroll-tools and read-steer observers never write the scroll position while the follow state is off.
- Browser walk covers the resting case at desktop width; unit pins keep covering the follow state's transitions.
- Validation: page-only selection (path-selected gate) green.

# Notes

Landed as df3a0210 (stack 29be52f7, 5abd4f57, df3a0210) on github/main, gate green 153 steps (owner tier, path-selected), deployed 2026-11-10 2:37 AM ET and passed its 15-min health watch (coordb). Closed on that confirmation. #1144 stays open for the owner's scroll-trace verdict.
