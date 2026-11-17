---
title: 'vibe-talk-setup-routes-and-pairing-ux: clear setup routes, pairing guide and first-run streamlining'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- docs
created_at: 2026-11-17T17:10:30.000000000+00:00
updated_at: 2026-11-17T17:10:30.000000000+00:00
---

# Description

Owner request (2026-11-17): optimise the initial setup and install of vibe-talk for new users who will try it on diverse machines and connect back from a desktop browser or a phone. Enumerate the connection routes that work, document install and first pairing clearly, and review and streamline the install and pairing flows for UX. Also answer whether the agent-utils-level setup flow is wizard-style and what it covers for vibe-talk.

# Design

Add a user-facing setup guide covering install (prerequisites, build or container, config, first start), then one section per connection route (same machine, LAN, SSH port forward, Cloudflare Tunnel, Tailscale, and a generic corporate dev-server web proxy), each with steps, security notes and trade-offs, then phone install as a PWA (Android and iOS) and pairing with the one-time pairing link (#369 pairing-link). Add a `vibe-talk doctor` setup check that reports configuration problems that otherwise surface only later (no durable storage, placeholder credentials, no public address for pairing, plain-HTTP exposure, no writable channel, live updates off, voice/summaries unconfigured), with a named fix for each. Record the UX review with one concrete fix per friction point; implement the small safe fixes in this change and list the larger ones as follow-ups.

# Acceptance Criteria

A new user can pick a route from the guide and reach a signed-in phone without reading the reference README end to end. `vibe-talk doctor` exits non-zero on a configuration that cannot serve the app and names the fix for every finding. The site page links the guide. No internal host names, account ids, workspace names or local paths appear in any added file.

# Notes
