---
title: 'channel-browser: discover and add named chat channels'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- channels
- ui
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T04:25:30.511019675+00:00
updated_at: 2026-09-25T04:25:30.511019675+00:00
---

# Description

Add a provider-neutral source-discovery API and Settings UI so the owner can browse named chat channels visible to the configured account, search the results, and add a selection to the app's tracked subset without knowing or pasting a provider resource ID. Provider-specific enumeration, pagination, identifiers, and authorization remain in the private adapter.

# Acceptance Criteria

Settings has a Browse channels action; the list shows stable display names for sources visible to the configured account; text search filters promptly; selecting a result adds it through the existing registration path and persists across restart; already tracked sources are identified and cannot be duplicated; pagination, empty, denied, loading, and failure states are usable on a narrow mobile viewport; public code and tests contain no provider-specific names, URLs, auth, or identifiers.
