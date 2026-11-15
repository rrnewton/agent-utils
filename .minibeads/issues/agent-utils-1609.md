---
title: 'isolated-probe-insteadof: the isolated remote probe honours the caller insteadOf and proxy environment'
status: open
priority: 2
issue_type: bug
external_ref: gh-304
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

Part (b) of the remote-respelling report; part (a), accepting another spelling of the same repository at the strict remote-digest comparisons, is fixed (#868 wrkslots-remote-url-equivalence, closed). The network remote-authority probe runs git ls-remote in an isolated Git environment whose only configuration is the fixed network config. That environment does not inherit the caller's url.insteadOf rewrites or proxy settings (commonly carried in GIT_CONFIG_PARAMETERS), so with an SSH-spelled remote the probe fails authentication and refuses slot create/unpushed, even though the caller's own Git would rewrite the URL and succeed. A separate diagnostic (#882) explains fetch failures caused by a user rewrite, but the probe still neither honours the rewrite nor skips the network when a fresh local fetch already proves reachability.

# Design

Either honour the caller's insteadOf/proxy environment in the isolated probe (carry the rewrite and proxy variables through explicitly, without inheriting the rest of user config, and keep publication proof judged at the recorded URL as #871 requires), or skip the network probe for a remote with a fresh local fetch. Whichever is chosen, a rewrite that was applied or ignored is named in the output, never silent.

# Acceptance Criteria

- With an SSH-spelled remote and a caller insteadOf rewrite to https, create/unpushed probes succeed through the rewrite (or skip on a fresh fetch) instead of refusing with an authentication failure.
- Publication proof is still judged at the recorded URL; a rewrite pointing at a different repository never counts as proof.
- The probe output or refusal names any rewrite applied or ignored.

# Notes

[watermelon-full-preview] Filed from GitHub #304 isolated-probe-insteadof (https://github.com/rrnewton/agent-utils/issues/304) during Request 098 triage. Part (a) of that report is fixed on main by f16969d21f894ff86e642b4342d3bd1b31550758 (#868); this issue is the remaining probe-environment half.
