---
title: 'copy-upstream-link: copy a message''s own link in Slack or Google Chat from its ⋯ menu'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-21T22:59:05.978491551+00:00
updated_at: 2026-10-22T01:44:41.891097352+00:00
closed_at: 2026-10-22T01:44:41.891097221+00:00
---

# Description

[muse watermelon] The owner, 2026-10-21: for either Slack or Google Chat a URL can be had for a message, and he would like to copy it from vibe-talk where it is accessible, as one of the options in a message row's ⋯ menu.

Behaviour wanted: where the server can form the message's own URL in the service it came from, the row's ⋯ menu offers "Copy link in <service>" (named for that message's provider). Choosing it puts the URL on the clipboard and confirms with the page's usual small notice, and says so when the browser refuses the copy, as Copy text does. Where no URL can be formed for a message, the item is absent — not shown disabled.

The URL is the message's own in the upstream service: a Slack permalink (a thread reply's link opens its thread at the reply), a Google Chat web-client link to the message (its thread where the message part cannot be formed), and a Discord channel link where the server (guild) is known. The server forms it from the parts it already holds — for Slack, the conversation and timestamps plus one workspace URL, configured or reported by the provider — and sends it on the message the page receives; the page does not guess service URL shapes itself. Google Chat's forms are confirmed from public sources: Google's own sample code builds https://chat.google.com/room/{space}/{thread}, Google's event payloads carry https://chat.google.com/room/{space} as a space's URI, and a system-assigned message resource name is spaces/{space}/messages/{thread}.{message}, which the web client links as /room/{space}/{thread}/{message}.

The keymap gains the same act where a link exists, and the ? help says it.

# Acceptance Criteria

A Slack channel message's ⋯ menu offers Copy link in Slack and the clipboard then holds its archives permalink; a thread reply's holds the permalink with its thread. A Google Chat message's menu offers Copy link in Google Chat with its room/thread/message link (thread or space link where the message part cannot be formed). A message with no formable URL shows no such item. Choosing the item confirms with the page's small notice, and a refused copy says so. The l key copies the highlighted message's link where it has one, and ? help lists it. Rust tests cover forming per provider (channel message, thread reply, missing parts give no URL); a Chromium walk covers the menu item present with a URL, absent without, the clipboard write, at 1280x800 and 393x852.
