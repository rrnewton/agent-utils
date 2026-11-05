---
title: 'voice-page-copy-link-key: Cmd-L copies the vibe-talk link'
status: closed
priority: 0
issue_type: feature
assignee: cl-refactor
labels:
- vibe-talk
created_at: 2026-11-05T15:17:20.313770248+00:00
updated_at: 2026-11-05T20:03:01.193941049+00:00
closed_at: 2026-11-05T20:03:01.193940899+00:00
---

# Description

[opus 5.5] OWNER REQUEST (2026-11-05, 10:16 AM ET). The owner's words, verbatim: "Let's ad Cmd-l for copying the vibe talk link instead of the slack link."

It is an owner-approved exception between #624 voice-page-modularity batches.

# Design

- Cmd-L on Mac (Ctrl-L elsewhere) copies the selected or current message's vibe-talk deep link, the same link as the ⋯ menu's "Copy vibe-talk link", not the Slack link.
- If an existing shortcut copies the Slack link, the vibe-talk link becomes primary. The Slack copy stays reachable from the menu, or moves to another documented shortcut.
- Browser constraint: in a normal browser tab, Cmd-L focuses the address bar and the page may not be able to take it. In the installed app (standalone display mode) it is free. Verify both.
  - If a tab cannot receive it, keep Cmd-L in the standalone app and add a tab fallback (for example Cmd-Shift-L, or the existing copy key).
  - Show the right hint per mode.
- Visible feedback: "vibe-talk link copied".
- Not active inside inputs or the composer.
- The shortcuts help and cheat sheet show the binding.

# Acceptance Criteria

- Tests cover:
  - the binding (and the fallback in tab mode, if needed);
  - that the vibe-talk link, not the Slack one, is copied;
  - the input/composer exclusion;
  - the help text.
- The tab-vs-standalone behaviour is verified and recorded in the resolution.
- Gate, land, and send the SHA to the lead for an immediate deploy.

# Notes

Relayed by the lead on 2026-11-05 (owner reply 161). Assigned to cl-refactor, ahead of further #624 batches.

## Resolution

[opus 5.5] Landed as 2fbb7bd0 on main (gate base 79b69472). Ctrl+L (Cmd+L) on the selected message copies its vibe-talk link through the ⋯ menu's own copyDeepLink, so the link format still changes in one place; the status line says "Copied the vibe-talk link." (the menu's existing words). Bare l and the menu still copy the Slack/service link. Ctrl+Shift+L (Cmd+Shift+L) does the same for a browser tab whose Cmd+L goes to the address bar, and the ? list names it outside the installed app (display mode read from a CSS custom property). It is never taken in a text field. The menu item carries aria-keyshortcuts. Tests: dom.test.mjs (binding, wording in both modes, repeats, rowkeys) and a done.mjs page test.

Unverified: real Chrome's routing of Cmd+L in a tab (headless CDP dispatch bypasses browser accelerators; the fallback covers it), and whether Safari takes Cmd+Shift+L for its sidebar first.
