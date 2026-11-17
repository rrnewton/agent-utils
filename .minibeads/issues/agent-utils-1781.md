---
title: 'vibe-talk-first-run-welcome-channel-default: Channel is the default view and a first-run Welcome explains the app'
status: open
priority: 1
issue_type: feature
assignee: mu-firstrun
labels:
- vibe-talk
created_at: 2026-11-17T12:00:00+00:00
updated_at: 2026-11-17T12:00:00+00:00
---

# Description

Owner request (2026-11-17): "I want a guided flow when you first open up the app that explains what is what. A welcome to vibe talk screen. I want the channel to be the primary view that loads not the voice view when you start the app. The guided setup section should guide you through adding your first channel. And we should have concise information highlighting the benefits and flow for using vibe talk."

Today the app boots on the Voice (call) view: `createScreens()` starts on `voice` and the boot calls `showView("voice")`; `reopenSavedView()` only moves to the Channel when a saved record already says `discord`. A fresh install therefore lands on an empty call screen even though the owner's daily work is reading channels. There is also no first-run explanation anywhere: Help documents each control in depth, but nothing gives a new reader the map of the app in a minute.

Build: (1) the Channel view is the default on every start path, honouring any saved choice or deep link; (2) a short first-run "Welcome to vibe-talk" screen explaining each surface in one or two lines, dismissible, never shown again unasked, re-openable from Settings and Help; (3) when no channel is configured, the welcome walks the reader through adding the first one through the existing Settings channel mechanism.

# Design

Default view: the screens owner starts on `discord` (Channel), and the boot's first `showView` follows the saved record (`voice` only when the record says so) so a returning Voice reader sees no flash and no change. `reopenSavedView()` keeps its saved-choice and `?v=docs`/deep-link precedence; pairing-link sign-in reaches the same default through `signIn` -> `reopenSavedView()`. PWA `start_url` is `/`, the same page, so no manifest change is needed.

Welcome: a new `welcome` screen beside sign-in/main/Settings/Help, shown once after first sign-in when this device has neither a saved UI record nor a dismissed-welcome mark (`vibe-talk.welcome.dismissed` in device storage). Its primary act dismisses to the Channel view. While the directory holds no channel it also carries the guided setup from the SETUP guide (`vibe-talk/SETUP.md`, #1780): add a `[[channels]]` entry (id + label + `writable = true` where replies should post) or extend `VIBE_TALK_CHANNELS`, restart, run `vibe-talk doctor`, then press Check again, which re-reads the channel list through the existing `/api/v1/client-config` path. The snippet stands in a `pre` with a Copy button. Channels come from the server's configuration today (in-app registration exists only behind a registration-capable bridge), so the Welcome creates nothing on the server. Settings gains a "Welcome" button next to "Open help", and Help's intro links to the same screen, so the copy lives in exactly one place.

Copy (one or two lines each, from the owner's observed flow: read a channel, follow a thread, catch up with Discuss, read linked Documents, talk hands-free in Voice, work the list with Read/Pace and Done/Undo, find anything with Search): Channel is home; Threads keep a topic together; Discuss catches you up; Documents read what chat links to; Voice is for talking; Read/Pace reads aloud at your speed; Done/Undo clears what you have handled; Search finds any message. No internal host names, ids or local paths.

# Acceptance Criteria

A fresh launch (no saved record, no welcome mark) signs in onto the Welcome screen, and dismissing it lands on the Channel view; a launch with a saved Voice choice still opens on Voice; a deep link (including `?v=docs`) still wins over the default. The welcome never appears for a returning reader who dismissed it, and re-opens from Settings and from Help. With no channel configured the welcome shows the guided config steps with a copyable snippet and a Check again that re-reads the channel list; with a channel configured it does not. Page tests cover phone and desktop sizes, light and dark themes, and the keyboard-open layout for the targeted states.

# Notes

Owner usage patterns (from the last 168 hours of owner prompts, ai_docs reports and issue history, summarised 2026-11-17): the owner lives in the Channel view — reading channels and threads, marking Done as they go, asking Discuss to catch them up on what arrived while they were away, opening linked Documents to read properly, and searching for a message or decision. Voice is the occasional hands-free front end to the same channels, not the home screen; Read aloud is paced listening over the same list. The welcome copy above follows that order: Channel first, Voice as one tool among several.

Coordination: the sibling lane documenting installation and pairing (#1780 vibe-talk-setup-docs) receives this welcome copy for reuse so first-run wording stays consistent.
