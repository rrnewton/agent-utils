---
title: 'vibe-talk-mention-link-not-clickable: a mention that renders blue must be a real link'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T11:17:02-05:00
updated_at: 2026-11-16T14:00:52-05:00
closed_at: 2026-11-16T14:00:52-05:00
---

# Description

[watermelon-full-preview] Owner report (fast-track, owner rule 286), 2026-11-16 11:13 AM ET, part of a bigger @-targeting TODO that a sibling design lane handles; this issue fixes only this bug: when a message with a working @-reference is copy-pasted, it looks like a profile link in angle-bracket pipe form, and renders blue and formatted as if special, but it is NOT a hyperlink. On Slack itself it is a hyperlink that opens the profile. The owner asks that, if possible, clicking the link opens Slack and the person's profile, if that is linkable.

In vibe-talk, a mention that renders blue/special must be a real link. The stored forms of mentions in the live cache (user mentions, profile links like the one above, and Slack rich_text user elements) must each render as a link: to the Slack profile when a profile URL is derivable (using the configured Slack workspace URL, or the profile link Slack itself gave), otherwise to the URL in the stored link. The link opens in a new tab/app and does not trigger message selection or card collapse. No popup/autocomplete work (that is the design lane).

# Acceptance Criteria

- Unit/page tests for each mention form produce one anchor with the right href and label: a profile-link form; a plain user mention with and without a known display name.
- Condition-based waits only (rule 346).
- Acceptance screenshots on a TEST instance (phone and desktop) showing a mention as a link (hover/underline state if possible); paths in the report and handoff; the liaison reviews.

## Resolution

[watermelon-full-preview] Landed on main as `2ac10a96` (with `a5a01740`): the server renderer threads the channel provider's configured Slack workspace origin (`render::profile_base_for`) into the mention chips, so a user mention renders as `<a class="mention" href="{workspace}/team/{user id}">` with the display name (or `@<user id>`) as its label; the sanitizer admits the mention class on anchors and gives every link `target="_blank"`. The stored profile-link form renders as an anchor to its address, as before. Channel chips and mentions with no derivable workspace stay chips. Live-cache census (read-only): stored forms are `<@U…>` user mentions in message text (Slack `rich_text` blocks are not stored; the text fallback is), profile URLs as bare addresses, and the angle-bracket pipe form in pasted text. Tests: render unit test `a_mention_with_a_workspace_is_a_profile_link_and_each_form_links` (each form one anchor with the right href and label; named, unnamed, no-workspace and code negative controls) and `mention_link_browser.py` (one anchor per form at 412x915 and 1280x800, hover underline, tap opens the profile in a new tab with no fold, scroll move or selection). Screenshot matrix (liaison-approved): /tmp/mentionlink-matrix, 9 images over light/dark x keyboard open/closed x phone/desktop with the Discuss half-pane open in the phone keyboard-closed shots, plus the before (dead span chip) shot. Full local validate green; owner gate green but for `ios-keyboard-send-browser` under load, cleared by its solo rerun. Deploy is coordb's (midnight ET batch unless the owner says deploy now).
