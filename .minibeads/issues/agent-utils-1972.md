---
title: 'agentctl-muse-hooks-pretrust: pre-trust our plugin hooks via Muse supported approve verb so the dialog never appears'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1932 agentctl-muse-hooks-trust-auto-decline (landed 62f03e1d). #1932 answers Muse's "Review plugin hooks" dialog per `muse_hooks_trust` at start/restore/revive/watch, and this registry sets `"trust"`. The owner (2026-11-20): "Can we do a better fix for that uniformly? And if not can we have an agentctl hack where we turn on a flag in agentctl to have its deterministic watchers skip past that menu with a yes-trust-hooks?" The owner approved trusting our own hooks on 2026-11-19. This issue asks whether Muse has a supported persisted hook-trust store so pre-trusted hooks never show the dialog at all; if so, agentctl should pre-trust exactly the hooks in play for its profiles at launch, hash-pinned so changed hooks still prompt, keeping #1932's auto-answer as the fallback. Never trust hooks that are not ours; never edit Muse's files in an unsupported way.

# Design

## Research findings (2026-11-20, Muse 1.4.25, scratch XDG home, real settings untouched)

- Yes: a supported store and verb pair exist. `~/.config/muse/settings.json` holds `runtime_capabilities`: a map from stable capability id (`plugin:<plugin>:hook:<hook-id>`, also `plugin:<plugin>:invocation_preapproval:skill:<name>`) to `{"enabled": bool, "trusted_definition_hash": "sha256:..."}`. The supported writers are `muse plugins approve <plugin-id[[:kind]:capability-id] | stable-id> [--json]` (trust and enable) and `muse plugins reject ...` (trust and disable). They exit 0/1 and emit JSON (`{"decision": "approve", "runtime_capabilities": [{"stable_id", "enabled", "trusted_definition_hash"}]}`; errors as `{"error": {"code", "message"}}`, e.g. `unknown-plugin`, `runtime-capability-not-found`). There is no separate `hooks trust` verb; this is it.
- `muse plugins inspect <id> --json` reports each candidate's current `definition_hash` and a status: `trusted_enabled` (hash matches, enabled), `review_needed` (no entry), `modified` (entry exists, hash differs — including reject-then-change), `trusted_disabled` (rejected at current hash). Disabled plugins report no candidates. The startup dialog's "N new or changed hooks need your trust" counts `review_needed` + `modified` hooks of enabled installed plugins; bundled plugins (herdr, muse-core, ...) never appear in `plugins list` and never prompt. Managed hooks are separate (`managed_hooks_path`, enterprise policy admissions with digests) and never prompt.
- Hash-pinning is real and stronger than the dialog's wording ("Approval follows the declaration, including after script updates"): in a scratch local plugin, changing only `timeoutMs` and separately changing only the hook script body each produced a new `definition_hash` and status `modified`. A changed hook therefore re-prompts (and #1932's fallback answers it), never silently inherits trust. Residual unknown: whether the hash covers files a hook command reaches outside its plugin package (absolute-path helpers); upstream source was not read, so the dialog fallback must stay regardless.
- Decisive scratch TUI probe (own tmux socket, scratch XDG, `--trust-workspace`): with the probe hook `review_needed`, the interactive launch showed the dialog in ~2s ("1 new or changed hooks need your trust"); after `muse plugins approve plugin:hooktest:hook:hello`, the same launch showed no plugin-hooks dialog and proceeded past that gate.
- Blanket `muse plugins approve <plugin-id>` trusts every runtime capability of the plugin, including `invocation_preapproval:skill:*` (verified: it re-approved a deliberately rejected skill preapproval). Agentctl must approve per-hook stable ids only, never the blanket form.
- `legacy_hook_trust_amnesty_applied` (top-level bool plus sentinel `runtime_capabilities` entry `tbh:amnesty-applied:v1, enabled: false`) is Muse's own one-time migration marker, not a pre-trust API; agentctl must not write it. `~/.config/muse/trust.json` is workspace trust, unrelated. Hand-editing `settings.json` would be the unsupported path the owner ruled out; the approve verb writes the same store under Muse's lock (`.settings.json.lock`, tmp+rename observed).
- Uniform-fix note: Muse's launcher already seeds trust for preset-installed plugins at first install in a fresh home (observed in scratch); the gap is updated definitions and non-preset plugins, which only the approve verb or the dialog can clear. That upstream gap is Muse's to close; agentctl can only pre-approve at launch.

## Proposed design

- Gate: only when the resolved #1932 policy is `trust` (per-profile > registry default > built-in `decline`). `decline` launches are untouched (no approve, no reject); `decline` is never persisted via `reject`, since that would disable hooks the dialog's option 3 leaves merely untrusted for the session.
- At each muse launch point #1932 already covers (start, revive, restart, restore; not the watchers), before the harness starts, run the launch's own `muse` binary with the launch's effective environment: `plugins list --json`, then per enabled plugin `plugins inspect <id> --json`, collect hook candidates with status `review_needed` or `modified`, and `plugins approve <stable_id> --json` for each. Approve only `kind: hook`; never skill/invocation-preapproval candidates; never a blanket plugin approve. Verify each approve's returned `trusted_definition_hash` equals the inspected candidate hash; on any mismatch, error, or missing binary, log and continue the launch unpinned (#1932's dialog answer remains the fallback — pre-trust failure never blocks a launch, and nothing is claimed pre-trusted that was not verified).
- "Ours" scope, one decision needed: profiles carry no plugin list, so the hooks in play for any muse profile are all enabled user-installed plugins (today all developed in-house; owner 2026-11-19: "we've developed our own hooks, no external ones"). Options: (a) approve for all enabled installed plugins (mirrors the dialog's Trust-all scope #1932 already applies); (b) a new optional allowlist key in profiles.json naming plugin ids (reported to the coordinator before landing, per the reserved-issue brief; both editions warn-and-ignore unknown profiles.json keys, so older readers stay safe). Recommendation: (a) for behaviour now, with every approved plugin/hook/hash logged, and (b) only if the coordinator wants a narrower blast radius.
- Hash-pinning story: approval pins the exact current definition; any later declaration or script change flips the hook to `modified`, the dialog returns, and #1932 answers per policy. A plugin update between pre-trust and pane start is the same race with the same fallback.
- Tests (fake `muse` plugins CLI + fake Herdr, scratch registries): decline sends no approve; trust approves exactly the `review_needed`/`modified` hooks by stable id and nothing else; approve failure still launches and the dialog fallback answers; a post-approve hash change reaches the dialog path. New test files classified in `validation/components.json`; `common/docs/agentctl/harness-support-matrix.md` Muse row updated in the same commit. Both editions (Python and Rust) or an explicitly scoped first edition, per coordinator direction.

# Acceptance Criteria

- Under `muse_hooks_trust: "trust"`, a muse launch whose hooks were updated shows no plugin-hooks dialog and delivers its brief; the log names each pre-trusted plugin, hook stable id, definition hash, and the setting's source.
- Under `decline` (or no setting), agentctl runs no approve/reject and #1932 behaviour is byte-for-byte unchanged.
- No blanket plugin approval is ever issued; skill and invocation-preapproval capabilities are never approved by this path; a hook whose definition changed after approval prompts again and is handled by the #1932 fallback.
- Pre-trust failure (missing/failing `muse`, hash mismatch) is logged, never blocks the launch, and never claims success.

# Notes

## Scope decision (coordinator, 2026-11-20)

Option (a), restated after the host reboot: approve per-hook stable ids for ALL enabled user plugins, only when the trust policy is "trust", each approval hash-verified against the definition Muse reports; no new profiles.json allowlist field. Keep #1932's dialog answer as the fallback. Cold review before gating: send the coordinator the SHA.

## Note: workspace-trust detector false positive (coordinator, 2026-11-20)

While delivering the research above, agentctl's trust-prompt detector fired on the receiving pane's own screen text (the findings quoted `pre-trust` / `trusted_definition_hash` and the scratch probe's quoted trust dialogs), and agentctl refused delivery with "Muse workspace trust prompt requires human attention". That is a false positive worth a regression test here or in a sibling issue: `muse_trust_prompt` in `py/agentctl/client.py` (and its Rust mirror) matches loose substrings ("trust this workspace", "do you trust" + workspace/folder, "workspace trust") over the whole screen, so dialog text quoted inside a working lane's transcript trips it. The hooks dialog already has the strict bottom-anchored recognizer (`_hooks_trust_block`, with composer and turn-marker vetoes); the workspace-trust recognizer has neither. Regression shape: a screen whose transcript quotes the workspace-trust dialog with a composer present must not be treated as a workspace-trust prompt, while the real bottom-drawn dialog still is.
