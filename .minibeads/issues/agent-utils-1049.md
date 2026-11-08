---
title: 'merge-queue-unattested-receipts: identity lines are opt-in until validators print them'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-08T07:58:31.000000000+00:00
updated_at: 2026-11-08T08:16:00.000000000+00:00
---

# Description

#1045 merge-queue-donor-tool-identity landed with a missing
`merge-queue-tool-identity:` line refused as identity-mismatch
for every full-hex tool stamp. That breaks the forward-bump
rule (an agent-utils bump must never break consumers): an
existing consumer queue whose validator does not print the
lines yet would, on upgrading merge-queue, refuse every donor
receipt and stall in decide mode, however healthy its tools.
Strictness must be opt-in after the safe rollout, not the
default of the bump.

# Design

- Default (`validator.require_tool_identity_lines` absent or
  false): a ran receipt with NO identity line for a configured
  full-hex tool is ACCEPTED. The acceptance is recorded, never
  silent: the donor writes a `tool_identity_unattested` warning
  on the receipt, the queue records the same on the job
  (visible in `donor status`, counted like `could_not_run`
  entries) and the run loop alarms it once per entry.
- Strict (`require_tool_identity_lines: true`, set by config or
  `init --require-tool-identity-lines`): missing means refused
  as identity-mismatch, exactly the #1045 behaviour. The field
  serializes only when true, so existing configurations and
  their validator digests are byte-identical until a consumer
  opts in.
- A PRESENT line is always held to #1045 in both modes: equal
  or provably descendant, or the receipt is refused. Malformed,
  extra and conflicting attestations also still refuse. Only
  absence is compat-relaxed. Non-hex stamps are unchanged
  (optional line, must equal when present); refused receipts
  stay exempt in both modes.
- USER_GUIDE documents the safe rollout order: teach the
  validator to print the lines first (older merge-queue ignores
  them), swap binaries, watch the unattested counter fall to
  zero, then enable `require_tool_identity_lines`.

# Acceptance Criteria

- Missing line + default: accepted, with the unattested note on
  the job, the warning on the receipt, and #1045's other tests
  undisturbed.
- Missing line + strict: refused as identity-mismatch, job
  stays leased.
- Present + mismatch (stale or unrelated): refused in both
  modes. Present + match (or descendant): accepted in both.
- An existing serialized config without the new key parses and
  digests exactly as before #1045's follow-up.

# Notes

Urgency: a consuming queue's binary swap depends on this; it
was briefly under the impression missing lines were already
optional and has been corrected — print-first ordering above
is safe under both behaviours. Filed by mu-archive; #1048 is
reserved locally by the parked merge-queue-terminal-verdict
issue commit, hence this issue numbers after it.

Landed by mu-archive in the commit closing this issue:
`require_tool_identity_lines` on `ValidatorConfig` (+
`init --require-tool-identity-lines`), donor `tool_warnings` on
the receipt, queue-re-derived `tool_identity_unattested`
counted on the job and alarmed by the run, strict missing-line
refusal only when opted in. Tests: missing+default accepted
with note/counter, missing+strict refused, present+mismatch
refused in both modes, present+match accepted, serialized
config omits the key until enabled.
