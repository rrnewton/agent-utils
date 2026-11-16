---
title: 'vibe-talk-empty-red-banner: an empty red banner bar at the top of the page'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-16T17:00:00.000000000+00:00
updated_at: 2026-11-16T17:00:00.000000000+00:00
---

# Description

Owner screenshot, 2026-11-16 11:56 ET (Android, light theme, keyboard open,
thread reply composer open, Discuss half-pane open): an empty red-outlined
banner bar at the very top of the page — pink fill, red border, no text, no
visible dismiss control (the right end sits under the tall Undo chip, which
is #1725 vibe-talk-stretched-undo-chip). Sibling lanes from the same
screenshot: #1724 Discuss panel ignores the light theme, #1725 the stretched
Undo chip. This issue is only the empty banner.

The page has exactly two banner/notice elements that can sit there, both
driven only by `web/src/view/shell/notices.ts`: the error panel
(`#error-wrap` / `#error`, `showError`) and the connection banner
(`#connection-banner` / `#connection-detail`, `showDetail`). Neither guards
its text: `showError` unhides `#error-wrap` and writes
`redact(text)` even when that is empty, and `showDetail` unhides
`#connection-banner` with an empty detail the same way. Callers that hand in
`error.message` (or a `detail || message` chain) can therefore put up a
coloured shell with nothing in it when a failure carries no words — an
`Error` with an empty message, or a server answer whose detail is empty
(`error.detail` is `""` when the body gave none, `web/src/core/api/client.ts`).

Live-log check (read-only, 11:50–11:57 ET window): no 4xx/5xx page request
explains a standing error in that minute beyond the Discuss reads; the
11:51 `POST .../catchup/update` answered 422 and the 11:56 reply posted
202/200. No server sentence was lost — the banner's text was empty on the
client side, so what it "meant to say" cannot be recovered from the log;
the fix therefore makes an empty notice impossible rather than guessing
its words.

Wanted:

- A banner or error panel never renders without text: `showError` and
  `showDetail` with empty or whitespace-only (or redacted-to-empty) text
  show nothing instead of an empty shell; a failure with no words of its
  own says so in words, following the page's existing "no reason was
  given" convention (`failureWords`).
- Existing behaviour otherwise unchanged: a real error still shows, stays
  for twelve seconds (or until dismissed with `stay`), and clears as
  before; the connection banner still shows its detail and takes itself
  away.
- Page tests at phone and desktop sizes for the exact defect, in light
  and dark theme, with the Discuss half-pane open in at least the phone
  shots, per the standing screenshot matrix.

# Resolution

Landed on main as 3d1baf69 (gated tree 41e8b4ee rebased on the #1729 fix
daaf2f7f). `notices.ts`: an error with no words reads "Something went
wrong, but no reason was given."; a connection banner with no detail
stays hidden; `addDetail` onto nothing leaves no separator; an empty
status line dismisses itself. Registered the new suite in
`vibe-talk/tests/page_modules.rs` (floor 405 to 409). Focused test fails
3 cases unfixed and passes fixed; local validate green on the exact
tree; the owner gate's only red was `vibe-talk.check.documents-browser`
timing out under load, which passed solo (13s) and in the one solo gate
rerun, and is recorded as a flake waiver by land-main. Screenshot matrix
in /tmp/mu-redbanner-shots: no empty banner in light/dark at phone or
desktop with the keyboard open or closed, and a real failure renders the
top panel with words and its dismiss control in both themes.
