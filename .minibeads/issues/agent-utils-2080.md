---
title: 'vibe-talk-summary-link-stays-in-view: a summary''s message link must stay in the current view, not jump to the thread view'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T17:53:00-05:00
updated_at: 2026-11-21T18:05:00-05:00
---

# Description

The owner (OWNER 305, 2026-11-21 5:53 PM ET): "CRAP I'm in the All view and got a big summary open.. I'm trying to click the messages in the generated summary. It is consistently jumping into the THREAD view which jumps AWAY From my index that I am navigating!!! I want to STAY on the discuss window and all view and just jump around the relevant messages. If i WANT I can click into a thread and drill in then press back."

The owner (OWNER 306, 2026-11-21 5:56 PM ET, same navigation code): clicking a `?c=<channel>&v=all&m=...` deep link made the Discuss panel refresh and come back the same, the main channel did not budge, and the message never loaded; after a reload it worked, but other links "don't seem to update my display and DO NOT satisfy the invariant that after clicking them a 'Linked Message' bubble APPEARS ON THE SCREEN".

# Design

Implemented rule (reported to the coordinator):

- A message link followed from inside a summary (citations, footer time links, bullet links) lands in the view the reader is already in, where that view can show the message: in All (flat) every message, replies included, is shown in place, so it never switches to the thread view; in the open thread its own messages land in place; in Main roots land in place and a reply falls back to All, the same global scope. Discuss stays open and the summary's own scroll position is untouched. If the message is not loaded in that view it is loaded in place (the seek walks older pages there, or All is read where the binding returns to the bound global view). Opening a thread stays an explicit action (thread card, thread strip, thread short-ref), and Back returns to the reader's previous position in the same view (the jump is one link-jump level, as before).
- Landed-or-reason contract at the shared jump layer (OWNER 306): every link kind — summary citations and footer times, `?m=` deep links (clicked and opened-by), quote links, issue and thread refs, Discuss links, message-body links — ends with the target message on screen with the "Linked message" bubble (`data-linked` marker) visible, or a persistent visible reason in the error panel naming the next step: "further back than what's loaded" with a Load older button that retries the landing, "couldn't be found in this channel", or, for a link this page's code cannot parse (minted by a newer server; the stale-page case), "newer than this page" with a Reload button. Never a silent no-op. Ordinary (non-summary) message-body links keep their `#543` behaviour (a reply link drills into its thread); the stay rule is the summary/deep-link one, the landed-or-reason contract is universal.

Root cause (305): the Discuss binding (`view/catchup/binding.ts` `linkDecision`) already answered jump-in-place for All, but the shared jump (`view/shell/navigation.ts` `jumpFromSpot`) then took its `ref.thread` branch unconditionally and arrived in the thread view; footer links bypassed the binding entirely.

Root cause (306): the jump had no landed-or-reason contract — a seek that gave up set only the fading status line and cleared the deep-link focus silently; `applyDeepLink` had bare silent returns (not-found after seek, an unreadable `m` degrading to channel-only); an unreadable `data-msg` mark fell through to the browser following the raw address.

Related earlier work not regressed: #1392, #1586, #1820, #1911, #543.

# Acceptance Criteria

- From a summary in the All view, every message link keeps the view and Discuss open, keeps the summary scroll, and lands the message on screen highlighted, including a reply that belongs to a thread and a message not yet loaded.
- The same stay rule holds in Main and in the open thread where those views can show the message.
- Every link kind lands with the "Linked message" bubble or shows the persistent reason with its next step; a stale-page link offers Reload.
- Back after a stay-in-place jump returns to the previous position in the same view; Back after an explicit thread open returns as before.
- Browser checks drive the real page and router (no stubbed navigation), phone and desktop; screenshots light and dark.

# Notes

Filed by the coordinator so the issue link resolves on main; refined by the owning lane (mu-summarylink) to cover OWNER 306 and the implemented rule.
