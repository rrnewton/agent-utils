---
title: 'root-is-the-app: serve the polished app at / and delete the old duplicative interface'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-14T16:11:59.845286450+00:00
updated_at: 2026-10-14T16:11:59.845286450+00:00
---

# Description

 Owner 2026-10-14: the root URL / still serves an old, partial, duplicative interface while the polished app lives at /voice. Make the primary app (today's /voice page) the page at /. Delete every piece of code, asset, route, test and doc that exists only for the old interface. Keep /voice as a forwarding pointer for now: a redirect to / that keeps the query string and fragment, so bookmarks and the installed PWA keep working (check the manifest's start_url and scope so an installed PWA still opens and stays in scope). Update the screenshot walk, browser checks, README and every link that names /voice.  (Reserved on main 2026-10-14; the work is on a branch and lands under this number.)
