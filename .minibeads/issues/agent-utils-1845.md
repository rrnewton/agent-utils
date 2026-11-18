---
title: 'vibe-talk-appearance-setting: add Settings Appearance Light Dark System'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
- ui
- settings
created_at: 2026-11-18T10:17:26-05:00
updated_at: 2026-11-18T10:17:26-05:00
---

# Description

[watermelon-full-preview] Owner request (thread 180): "By the way, I notice we correctly switch light/dark mode when I change my mac os system settings with the installed chrome PWA version of this webapp. But we should also have our own Appearance = Light | Dark | System in the settings."

Today the page follows `prefers-color-scheme` only (`web/style.css`, `web/styles/voice/00-frame.css` and the other light blocks). There is no in-app choice, no override, and no `theme-color` meta to keep the installed PWA chrome in step.

# Design

- Settings gains an Appearance control with three choices: Light, Dark and System. System is the default and keeps today's behaviour, following `prefers-color-scheme` live, including a media-query change while the page is open.
- Light and Dark override the system choice. The effective theme is applied as `data-theme="light|dark"` (and `data-appearance` for the stored choice) on the document element, with `color-scheme` set to match so form controls and scrollbars follow.
- The choice is a synced client setting (`vibe-talk.voice.appearance`, in `SYNCED_SETTING_KEYS` and the server's `SYNCED_KEYS`) so it follows the owner across devices like other synced settings.
- No flash: a marked pre-paint script in the shell head, before the stylesheet links, reads the stored choice, resolves System against `matchMedia`, and sets the initial `data-theme` before the stylesheet paints. The main bundle re-applies the same resolution on load and on every change.
- The PWA `theme-color` meta is kept in sync with the effective theme (dark `#14161a`, light `#f6f7f9`, the `--bg` values).

# Acceptance Criteria

- Each choice applied: Light forces light, Dark forces dark, System follows the system, verified in the browser at phone and desktop sizes.
- System follows a media-query change without a reload; Light and Dark do not.
- The persisted choice survives reload, including the pre-paint script setting the initial theme before the stylesheet paints (checked from the served shell: the init script stands before the stylesheet links).
- The `theme-color` meta matches the effective theme after each change.
- Screenshot matrix on a test instance: light and dark, keyboard open (really shrunk) and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus Settings with the control and a System toggle, each image inspected.

# Notes

Owner-priority gate; deployed on landing.

## Resolution

Landed: Settings > Messages > Appearance (System / Light / Dark, default System) as the synced
setting `vibe-talk.voice.appearance`; effective theme on `data-theme` with inline forced colours,
live `prefers-color-scheme` following under System, a pre-paint script in the shell before the
stylesheets, and the `theme-color` meta kept in sync. Verified on a real test instance
(--fake-discord): each choice applied, System followed an emulated media change, Light held,
the server held the synced value through the real client-settings route, and the persisted
Dark survived reload under a light system; screenshot matrix inspected.
