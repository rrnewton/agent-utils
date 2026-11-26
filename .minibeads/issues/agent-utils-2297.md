---
title: 'vibe-talk-settings-header-layout: the Settings header buttons fill the column, no stray side notes, and a short update chip'
status: open
priority: 1
issue_type: task
assignee: cl-settingshead
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T11:00:00-05:00
updated_at: 2026-11-26T11:00:00-05:00
---

# Description

The owner, 2026-11-26, of a phone screenshot of the top of Settings (dark theme): "these AI generated uis often seem very bad or unintuitive on first try. They litter verbose prose in places that a human would never put it in a UI. And simply as a layout thing. If you look at this screenshot it's got three buttons. I don't think it carefully designed UI would ever have three buttons like that right centered and not filling the column. They would probably either be centered or would fill the column."

What the screenshot shows at the head of Settings:

1. The three page-level buttons, Expand all, Welcome and Open help, sit in a row aligned to the column's end (`margin-inline-start: auto` on `.settings-head-actions`, from `#372 settings-header-layout`), so on a phone they leave a gap at the start and do not fill the column. End alignment is the convention for a dialog's actions, not for page-level controls.
2. The Screen and keys section carries a stray "(client side)" note (`.settings-scope-note`, from `#457 settings-server-side`) on its own line under its title.
3. Beside the server name, a full sentence reports the build: "This page is build f2c5af26; the server now serves 19fe934a. Reload to run it." (`#529 page-build-stale-notice`). It wraps over two lines and is prose where a status belongs.
4. The server name ("Server" plus the host) is cut with an ellipsis but competes with the build sentence for the line.

# Design

Settings header only; no other Settings change.

1. The three buttons become one row that fills the settings column, each button the same width (a three-column grid or equal flex items), on phone and desktop alike (or list-style rows, if the coordinator prefers that on a phone). Their order, ids and actions do not change.
2. Drop the "(client side)" note under Screen and keys (the markup and its now-unused CSS rule).
3. The build sentence becomes a short status chip in the server line: "Update available · Reload" while the server serves a newer page build than this page; tapping it reloads the page (the same action as the existing `#page-update` chip). The detail (both builds) stays reachable behind it as the chip's tooltip and accessible description, and the plain build line moves to Storage and connection. When the page is up to date the header shows nothing for the build.
4. The server label keeps its place and truncates cleanly on one line: the label never wraps, the host name ellipsizes and the port stays whole.

# Acceptance Criteria

- At phone (412 px) and desktop (1280 px) widths, in light and dark theme, the three header buttons are equal width (within 1 px) and together with their gaps span the settings column's content width (within 1 px).
- No "(client side)" text under Screen and keys.
- With a newer server build, the header shows a chip whose visible text is short (at most about 24 characters), and a tap on it reloads the page; both build ids are in its tooltip and in Storage and connection. Up to date, the header shows no build text at all.
- The server line stays on one line with the host truncated by an ellipsis and the port whole.
- A page test asserts the button row geometry and the chip text length at both widths; screenshot matrix (light and dark, phone and desktop, update-available and up-to-date) reviewed before landing.

# Notes

- Owner request 388, 2026-11-26 10:55 AM ET.
- Prior work: `#372 settings-header-layout` (the one-row head, buttons at the row's end), `#529 page-build-stale-notice` (the build sentence), `#457 settings-server-side` (the client-side note).
