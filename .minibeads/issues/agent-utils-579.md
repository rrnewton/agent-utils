---
title: 'voice-send-confirm-timing: the agent says a send failed or timed out the moment the card appears'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-10-27T13:10:15.878426049+00:00
updated_at: 2026-10-27T14:03:50.906848409+00:00
closed_at: 2026-10-27T14:03:50.906848309+00:00
---

# Description

[opus 5.5] Owner, 2026-10-27: "I will say send X on channel Y. The card will pop up but the agent will just immediately say that it wasn't sent because the confirmation timed out or something. ... it was false because I could still press send on the card and it actually went through."

Found: the voice page judges every final turn of the owner's that says "send" as a possible answer to the card. The owner's own request ("send X on channel Y") is one, so the page told the agent "no draft is waiting; the last one lapsed" (an earlier draft in the call had been settled) or "the message was NOT sent: only a plain answer counts" (a card already up), at the moment the agent was proposing the new draft. The agent repeated that as a timeout. Also: with the ElevenLabs agent set up as the README says (post_reply on Require approval), the vendor asks the client to approve each post_reply call and the page never answered, so the call timed out at the vendor; and a draft that really lapsed was never told to the agent.

Want: the agent says the message is on the card waiting for the user, never failed or timed out; a tap on Send or Don't send, or a real lapse, is told to the agent on both protocols; the page's not-sent word is never given for a turn that was a new request.
