---
title: 'provider-namespace-per-key: two accounts of one chat service can both add channels from the app, each keeping its own account'
status: open
priority: 3
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T08:28:21.966199236+00:00
updated_at: 2026-10-19T08:28:21.966199236+00:00
---

# Description

[opus 5.5] Follow-up to #355 provider-grouped-channels, which refuses this case with a reason.

Two accounts of one chat service in one server (two Slack workspaces at the same API base, or two Discord bots) get the same registration namespace, because the namespace is derived from the service kind and API base. A channel added from the app is stored under that namespace, and on every reload of the channel list the namespace is mapped back to the FIRST configured provider that has it. So a channel added for the second account moves to the first account at once (the server re-derives every route after the add) and after each restart, and its reads and posts then go to the wrong account.

#355 provider-grouped-channels refuses adding from the app for such a second account and says why; channels for it can still be listed in the configuration file.

Wanted: store the provider key beside each added channel (a store migration, with old rows falling back to the namespace as today), so both accounts can add channels from the app and each channel keeps its account across a restart. Then lift the refusal.

Tests: two providers with one namespace; a channel added for the second is read through the second at once and after a restart; an old row with only a namespace still resolves as today.

The same stored key would also settle two limits #355 provider-grouped-channels leaves, found in its review:
- Since #355, a channel added by id among several services records its provider's namespace, as a registered channel does. If that provider is later taken out of the configuration, removing the channel is refused (`channel_provider_mismatch`, "restore its original bridge configuration") although nothing upstream exists for a channel added by id; the page shows it under "Not connected" with Remove offered. Recording that the row was added by id would let it be removed locally.
- A channel added by id while the server read one service has no namespace. Once the server reads several, re-adding it by id is refused as "already in the list under a different registration policy". The channel itself still reads and posts through the default provider, as before. Old rows with no namespace should be treated as the default provider's when an addition names that provider.
