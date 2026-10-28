---
title: 'composer-no-autofill: no password manager offers to save on a send'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- web
created_at: 2026-10-28T07:45:43.301311704+00:00
updated_at: 2026-10-28T08:54:27.010629455+00:00
closed_at: 2026-10-28T08:54:27.010629144+00:00
---

# Description

[opus 5.5] The owner, 2026-10-28 on Android: "This wasn't happening before. I get this last pass suggestion every time I send a message on Android" (Android's 'Save username, email address, and password to LastPass' sheet over the page). Cause: the page has no <form>, so Chrome treats every control outside one as a single synthetic form; that form held the sign-in screen's type=password write-token field (hidden, but in the DOM, and filled with the stored token at every load) together with every message box. In Chrome's third-party autofill mode on Android the whole form goes to the phone's autofill service as a login form, and Chrome infers a submission when a fetch has succeeded and the field last typed in is removed or hidden (components/autofill/content/renderer/form_submission_tracker.cc, CanInferFormSubmitted / ElementDisappeared); it then calls AutofillManager.commit(), and LastPass shows its save sheet. Fix: the sign-in fields get their own form (form attribute, method=dialog, submit taken), the token is no longer kept in the password field once saved or at load, and every text field carries autocomplete=off plus data-lpignore / data-1p-ignore / data-form-type=other. Verifiable only on the owner's phone.
