---
title: 'settings-long-message-share-inputs: type the first-share numbers in one sentence'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-15T12:35:00.000000000+00:00
updated_at: 2026-11-15T12:35:00.000000000+00:00
---

# Description

Owner UI request 084 (2026-11-15): the Settings control "Not in the first share of a long message" should be two typeable text boxes in one sentence, `Not in the first [50]% of a msg over [32] words`; the sliders are bad. The sliders were added by #1537 mark-read-on-reply-settings (owner defaults set 2026-11-13 6:59 AM ET: first 50 percent, over 0 words, every number adjustable).

# Design

- The two range sliders and their stepper buttons become one inline sentence in Settings: "Not in the first [NN]% of a msg over [NN] words", with two `type="text" inputmode="numeric"` fields (percent 3ch, words 4ch). Values, bounds (0-100 percent, 0-5000 words), defaults (50, 0), persistence and server sync are unchanged (`clampMarkReadFirstPercent` / `clampMarkReadLongWords`, same device keys).
- Fields commit on change/blur/Enter (Enter blurs, so the change lands once). Out-of-range input snaps to the nearest bound and marks the field `aria-invalid`; an emptied or non-numeric field restores the previous value and keeps nothing new. Each field keeps its `aria-label` and describes the one combined saved line.
- The guard's toggle stays; both fields disable (greyed) while it is off. The sentence fits one row at 390px and wraps between words at 320px, a field never split from its unit, in light and dark.
- `voice.js` and `web/src/app/main.js` are untouched; the view interfaces keep their step members for the legacy wiring, which simply never fires them now.

# Acceptance Criteria

- Settings shows the one sentence with the kept values; no slider or stepper remains for these two numbers.
- Typing a number and pressing Enter or leaving the field keeps it (locally and synced, as before); past a bound it snaps with a visible invalid mark; empty restores the previous value.
- The guard off disables both fields; on re-enables them. No sideways scroll at 390px or 320px; the sentence is one row at 390px.
- Unit tests pin commit/clamp/restore/disable; a Chromium walk at phone and desktop widths pins the visible behaviour.

# Notes

Filed 2026-11-15 7:35 AM ET for lane mu-sharetext. Sibling lane mu-discordmerge (#1557) owns `web/src/present/rows/grouping.ts`; this change touches the Settings view and, only for its saved-line wording, `web/src/view/settings/preferences.ts`. No `present/rows` file changed, so no coordination was needed.
