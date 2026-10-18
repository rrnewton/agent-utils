---
title: 'pwa-app-name: let each deployment name its installed app'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-18T12:40:57.758312447+00:00
updated_at: 2026-10-18T12:53:54.333588123+00:00
closed_at: 2026-10-18T12:53:54.333587913+00:00
---

# Description

Owner, 2026-10-18: "If I install multiple pwa it gets confusing. Can you make it so each deployment can customize the name of the app". Two deployments installed on one phone both appear as "vibe-talk", in the launcher, the app switcher and the browser tab. Add a [server] app_name key, plus an optional short_name that defaults to app_name, with environment overrides VIBE_TALK_APP_NAME and VIBE_TALK_APP_SHORT_NAME; default "vibe-talk", so a deployment that sets nothing serves exactly the manifest it serves today. /manifest.webmanifest carries the configured name and short_name (id, start_url, scope and every other member unchanged, so an installed app keeps its identity); the page title, the sign-in heading and apple-mobile-web-app-title use it too, and client-config exposes it so the page can use it. A bad value (empty, too long, a control character) stops the server with a config error naming the key. Document it in the README configuration table and installing section (two installed apps from two deployments are told apart by this name; an installed Android app may need reinstalling to pick up a new one) and in vibe-talk.example.toml.
