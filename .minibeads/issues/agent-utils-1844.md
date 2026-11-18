---
title: 'vibe-talk-discuss-followup-422: follow-up questions in the Discuss panel return 422 non-JSON'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- discuss
- ui
created_at: 2026-11-18T10:00:00-05:00
updated_at: 2026-11-18T10:00:00-05:00
---

# Description

[watermelon-full-preview] Owner report (thread 180): a follow-up question in the Discuss panel, including a "find the message where ..." query, fails immediately with "vibe-talk returned non-JSON (HTTP 422)" shown at the top of the Discuss panel. Liaison log findings: every POST /api/v1/channels/*/catchup/messages has returned 422 in 0 ms and has never succeeded live, which points at the request extractor rejecting the body before the handler runs. The page sends {thread_id, text} from the catchup view; the server struct is {thread_id: Option<String>, text: String}. The error text is also not styled as an error.

# Design

- Reproduce against the real router on a test instance, find the exact request mismatch, and fix it so follow-ups and "find the message that ..." questions work end to end; the answer should cite or link the found message where possible.
- Every error on this route, and ideally all /api routes, returns JSON so the page shows the real reason instead of a non-JSON fallback.
- Discuss errors are styled as errors (red text, role=alert or an error class, both themes).
- No change to what a successful follow-up does beyond answering: confirm and report its side effects before any live check.

# Acceptance Criteria

- An end-to-end test posts the page's exact request through the real router and gets a 200 with an answer, stubbing only the model backend.
- A malformed body gets a JSON 4xx, never a plain-text rejection.
- A "find the message that ..." question returns an answer that cites or links the found message where possible.
- Discuss errors render in red with role=alert or an error class in light and dark themes.
- Screenshot matrix on a test instance: light and dark x keyboard open (real visualViewport shrink) and closed x phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the targeted error state before and after.

# Notes

Owner-priority, deployed on landing. Liaison plans one live read-only check after the deploy; the follow-up route's side effects must be answered to the coordinator before that.
