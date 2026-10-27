---
title: 'open-in-chat-jump: Open in channel on a sent card jumps to the message, switching channel and view'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-10-27T13:55:19.847305871+00:00
updated_at: 2026-10-27T14:03:50.910401340+00:00
closed_at: 2026-10-27T14:03:50.910401199+00:00
---

# Description

[opus 5.5] Owner, 2026-10-27: "This button here for open in chat is almost awesome but then it fails ... I want to click on it and jump to the channel and send another follow-up message or start a thread from there. Obviously when I click it I want to jump directly to that message. But instead I see a little chip pop up that says the messages in the zero 14 channel and I have to manually switch over ... I suggest that the all view with all chronological messages be the default. If I press the button I jump there and the screen is centered and highlighted on the message that was linked."

Today openSentMessage only works when the channel picker already shows the card's channel, and otherwise says to choose it. Want: Open in channel on a frozen sent card switches to the channel view, changes channel, opens All (chronological) by default, and seeks, centres and highlights the message, reusing #543 in-app-message-links' jump (one Back level, which returns to the call's view). The view it lands in is a setting: All or Main.
