---
title: 'signin-flash: a reload with a saved token shows the sign-in form for a moment'
status: closed
priority: 2
issue_type: bug
assignee: cl-urlchip
created_at: 2026-10-28T12:47:57.554468066+00:00
updated_at: 2026-10-28T12:57:51.798040055+00:00
closed_at: 2026-10-28T12:57:51.798039945+00:00
---

# Description

[opus 5.5] The owner, 2026-10-28: "whenever I refresh the page on desktop I would see the sign in form for just a second even though I was already signed in." Two causes, both reproduced in Chromium: (1) web/voice.html serves #screen-signin visible, and the page paints it while the classic /voice.js at the end of <body> loads and compiles (90 frames drawn with sign-in up over a 1.5 s script load); (2) with a token but no saved shell to hydrate from, the startup leaves that screen up until /client-config answers (95 frames over a 1.5 s answer). #616 composer-no-autofill (8e0c7909) moved the sign-in fields into a hidden form and changed no screen visibility: it neither caused nor fixed this. Wanted: with a token stored, the sign-in view is never drawn during load; it shows only with no token or after a refusal.
