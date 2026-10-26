---
title: 'slack-format-env-only: AGENTCTL_SLACK_FORMAT without a config key is reported but not applied'
status: closed
priority: 2
issue_type: bug
created_at: 2026-10-26T20:16:38.138615642+00:00
updated_at: 2026-10-26T20:16:52.632043492+00:00
closed_at: 2026-10-26T20:16:52.632043381+00:00
---

# Description

[opus 5.5] Found during the #530 slack-outbound-format live run. With no slack_format or slack_format_by_channel key, the bridge names no format on the wire, so AGENTCTL_SLACK_FORMAT alone changes nothing (and the helper's environment allowlist normally drops it), yet chat status reported the variable's value as slack_format. Decision: keep the variable an override of a configuration selection (so the wire stays v1 for non-Slack helpers) and make status report only what the bridge names on the wire: slack_format null when no key selects one, slack_format_source (config, environment, helper) and slack_format_environment_ignored.
