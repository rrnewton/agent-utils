---
title: 'agentctl-muse-hint-wordwrap-clip: word-wrapped Muse hints on narrow panes still read as staged'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-19T00:00:00.000000000+00:00
updated_at: 2026-11-19T00:00:00.000000000+00:00
---

# Description

Residual of #1888 agentctl-muse-placeholder-hint-reads-as-staged, live after its fix landed: on a narrow Muse pane (42 columns) the empty composer shows only "Paste an image with Ctrl+V — file" followed by the separator — the table hint "Paste an image with Ctrl+V — file paths and URLs work too" word-wrapped/clipped at the pane width, with only its first row drawn (33 of 58 characters, ending at a word boundary, 7 short of the frame edge). #1888's bare-prefix rule needs at least 34 characters and two-thirds of the hint, so this still reads as a staged draft and every queued message to the lane is held (drain: "did not become idle/done ... last status=staged"). Another lane holds the same way on "Ask to monitor a running build or log" (37 of 63). The styled reads already know the truth (the hint is drawn in the placeholder grey); the plain-text readers that status and drain rely on do not.

# Design

In both editions, a single-row editor whose whole text is a word-boundary prefix of a known hint also reads empty when the hint's next word would cross the frame's inner edge: that maximal word-aligned clip is how Muse draws a wrapped hint, as opposed to a person stopping mid-row with room to spare. The existing floors stay: at least 20 characters, and the mid-draw bare-prefix rule (34 chars, two-thirds) is unchanged, so typed opening words in a wide pane still read as a draft and are never typed over. Fixtures exercise widths 30, 42 and 80 for every hint in the table (at a width where a hint's clipped prefix cannot reach 20 characters, that hint simply cannot present recognisably and is skipped there).

Recovery is the same as #1888: held prompts were never typed, so the next drain by a fixed build delivers each exactly once.

# Acceptance Criteria

- At widths 30, 42 and 80, every table hint clipped at a word boundary at its frame's inner edge reads as an empty composer in both editions.
- A hint prefix stopping well short of the frame edge in a wide pane still reads staged in both editions.
- Focused suites pass in both editions.

# Notes

Follow-up to #1888 at the coordinator's request. No lane, host or person is named here.

## Resolution

[watermelon-full-preview] Landed 2084114d on github/main (owner-priority gate passed; combined tree with main verified: Rust muse_editor 27, Python launch hint/wrap 6). Both editions read a single-row, word-boundary hint prefix as empty when the hint's next word would cross the frame's inner edge (`is_hint_prefix` clip case / `_muse_is_hint_prefix`), with no threshold lowered; widths 30/42/80 fixtures cover every table hint. The affected lane's captured frame reads idle, and its real-CLI status flipped staged to idle under the fixed build with no typing. Held prompts were never typed, so the next drain delivers each once. First filed as #1894, renumbered to #1902 on the coordinator's notice (the earlier number was already taken on main).
