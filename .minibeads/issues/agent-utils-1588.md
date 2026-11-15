---
<<<<<<< HEAD
title: 'agent-teams-relay-discord: carry agent-teams cross-host messages through a Discord channel like the Slack bridge'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-15T16:51:49.921963859+00:00
updated_at: 2026-11-15T16:51:49.921963859+00:00
=======
title: 'autoplay-follow-scroll: smooth-scroll so the highlighted part stays in view during autoplay read-aloud'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-15T16:13:00.000000000+00:00
updated_at: 2026-11-15T16:13:00.000000000+00:00
>>>>>>> d92c0eb5 (vibe-talk: smooth-scroll to follow the read-aloud highlight under autoplay)
---

# Description

<<<<<<< HEAD
## Goal
Route agent-teams cross-host messages through a Discord channel (#agent-teams-relay, channel id 1566504459499217097) the same way they already route through the Slack bridge. The owner wants the DeepScry teams' hosts to use it: newton-fedora's bot is DeepscryCoder2 (user id 1545796614826889226), and the peer host's bot is DeepScryCoder1 (user id 1532416065114607829).

## What already exists
- rs/agent-teams/src/relay.rs is provider-neutral: "A provider only has to report the poster's stable sender ID and let a bridge post into the channel; Slack, Discord, or the deterministic fake all qualify". Envelopes travel in a fenced JSON block, receipts are posted in the envelope's thread, and ledger `accept` dedupes.
- The chat bridge has a Slack provider. `agentctl capabilities` lists `chat_subscriptions.reference_backends.discord-gateway` as implemented:false, with "gateway-subscription" and "live-end-to-end-verification" missing.

## What is missing
1. A Discord chat provider for the chat bridge:
   - subscription: Gateway, or REST polling as a first increment;
   - stable sender ids (Discord user ids);
   - posting into the channel and into a thread or reply for receipts;
   - escaping/unescaping for Discord markdown (the counterpart of undo_slack_link_markup).
2. Configuration: `agentctl chat init` for a Discord target, using a bot token from a file (never logged), the channel id, and the allowed peer bot ids.
3. Addressing (measured 11/15). Our bot wrongly ACKed an owner message that mentioned only the OTHER bot.
   - Admission must key on structured envelope targets and the bot's own user id or aliases, never on "I can see it".
   - Messages addressed only to another bot get no ACK. Duplicate and loop protection apply bot-to-bot.
4. A live end-to-end check between two hosts, plus a deterministic fake-provider test.

## Evidence
On 11/15 a Codex agent on newton-fedora read and posted to channel 1566504459499217097 through the Discord REST API with the existing bot token (HTTP 200). Bot permissions in that channel are therefore fine.

## Related
- agent-utils-496 agentctl-team-messaging (closed)
- agent-utils-1490 agent-teams-relay-to-queue-delay (open)
- agent-utils-1553 codex-footer-composer

A newton-fedora agent is drafting a first cut on branch `discord-relay-transport` (draft PR in this repo; link to follow). The owning team should treat that as input, not as a design commitment.
=======
Owner UI request 097. Autoplay (read-aloud) proceeds to the next unread
message when a read finishes, but the screen did not scroll to follow the
highlighted bit. The screen now smooth-scrolls so the highlighted sentence —
or, with no sentence light, the row being read — stays in the middle third
of the visible list, both when autoplay moves to the next message and as
the highlight advances within a long message.

# Design

`vibe-talk/web/src/view/voice/followscroll.ts` holds the pure rule (middle-
third band, centre-on-exit delta, reduced-motion choice, pause state);
`read-aloud.ts` supplies live rects and scrolls `#scroll-area` by the delta
with `behavior: "smooth"` (`"auto"` under `prefers-reduced-motion`). Hooks:
`showReadingSentence` (sentence advance) and `showReadingPosition` (block
boundary, which is where a new autoplay message starts). The port is
`#scroll-area` as laid out — with Discuss docked it ends at the panel's top
edge (#1526), so no panel-specific math is needed. Pause rule: a scroll the
follow did not cause pauses following until the highlight is back in the
band, the next block boundary, or another read starts; the follow's own
scrolls never pause it, and nothing scrolls on a timer, so a reader
dragging or flicking is never fought. No `voice.js` / `src/app/main.js`
edit: the highlight and read-start signals already live in `read-aloud.ts`.

# Test report

Unit: `tests/js/view/voice.test.mjs` gains the band/delta/behaviour/pause
cases. Browser: `tests/autoscroll_follow_browser.py` serves the page with a
fake local-voice API at 390x844 and desktop sizes, turns Autoplay on, and
asserts by condition wait that a later message is read with its row in the
middle third and the list scrolled. Screenshots from that walk are
inspected before landing.

# Notes

[opus 5] Implemented on branch `mu-autoscroll/1587` from `github/main`.
>>>>>>> d92c0eb5 (vibe-talk: smooth-scroll to follow the read-aloud highlight under autoplay)
