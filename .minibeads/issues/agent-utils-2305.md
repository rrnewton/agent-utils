---
title: 'vibe-talk-page-route-page-build: the /page route carries no page_build, so on an unthreaded server neither reload chip ever appears'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-26T12:00:00-05:00
updated_at: 2026-11-26T12:00:00-05:00
---

# Description

Found by #2297 vibe-talk-settings-header-layout while photographing its update chip.

The page learns which page build the server serves only from timeline answers (`TimelineResponse.page_build`, `#529 page-build-stale-notice`, noted by `noteServerPageBuild` in web/src/view/channel/readcore.ts). The newer `/api/v1/channels/{channel_id}/page` route answers with `PageResponse` (src/http/api.rs), which has no `page_build` field. On an unthreaded server every channel read goes to `/page`: the entry read (`reason=enter`), the polls (`reason=poll`) and the older pages (`reason=older`). So a page left open across a deploy on such a server never hears the server's new build, and neither the floating "New version — reload" chip (`#page-update`) nor Settings' "Update available · Reload" chip (`#settings-page-update`, #2297) ever comes up there.

On a threaded server (the owner's chat service) the channel is read through `/timeline`, which carries the build, so both chips work. The screenshot harness's throwaway server is unthreaded, which is why scenes 155 and 156 route the real server's client configuration to say the channel has threads (`_threaded_config`).

# Design

1. Add `page_build: Option<&'static str>` to `PageResponse`, filled with `crate::http::assets::page_build()` on every answer, exactly as the timeline handlers fill it, with `#[serde(default, skip_serializing_if = "Option::is_none")]` so an older page ignores it.
2. Regenerate the contract (types and validators) so the page's `PageResponse` type carries the optional field.
3. Have the page note the build from a `/page` answer through the same `noteServerPageBuild` path the timeline folds use, for every reason (enter, poll, older).
4. Tests: a server test that the `/page` answer names the page build; a page test that a `/page` answer naming another build shows both chips and that one naming the same build shows neither; and an end-to-end check through the real router on an unthreaded server (a page left open, the build changed, the next poll brings the chip up).

# Acceptance Criteria

- `GET /api/v1/channels/{id}/page` names the page build the server serves, on every answer.
- On an unthreaded server, a page whose build differs from the server's shows `#page-update` and Settings' update chip after its next `/page` read; a page on the same build shows neither.
- The threaded path is unchanged.
- Screenshot scenes 155 and 156 can drop the `_threaded_config` route and still reach both states through the real `/page` reads.

# Notes

- Pre-existing; not caused by #2297. Reported by cl-settingshead, 2026-11-26.
