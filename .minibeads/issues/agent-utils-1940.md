---
title: 'wrkslots-per-remote-ssh-command: a declared per-remote SSH command for reaching GHE'
status: closed
priority: 2
issue_type: feature
labels:
- wrkslots
- ssh
created_at: 2026-11-20T06:00:00-05:00
updated_at: 2026-11-20T14:00:00-05:00
closed_at: 2026-11-20T14:00:00-05:00
---

# Description

[watermelon-full-preview] A consumer's provenance harness uses a local mirror instead of GHE because wrkslots has no way to declare how to reach GHE over SSH (IdentityAgent=none, a specific key, a ProxyCommand). wrkslots should accept a declared per-remote SSH command in its isolated network repository.

# Design

- New optional config key `remote_ssh_commands`: a map from remote name to structured fields (`identity_file`, `identity_agent`, `proxy_command`). Paths to keys only; no secrets or key contents in config. Projects list the key in `ignorable_keys` so old readers ignore it with a warning and fail loudly exactly as today (reader-skew report approved by coordb 2026-11-20).
- Applied only as GIT_SSH_COMMAND for that remote's fetch/ls-remote/push in wrkslots network operations; never written to any Git config and never global. Every interpolated value is shlex-quoted, and paths or commands carrying shell metacharacters or newlines that could break out of the quoting are refused.
- Doctor reports configured remotes with identity_file path and proxy presence, never file contents.
- Python and Rust editions parse the key identically; Rust network operations honour it on the same paths.

# Acceptance Criteria

- A project config with `remote_ssh_commands` for its GHE remote loads; a fake SSH remote (local sshd-free harness using a declared identity/proxy) is fetched through the declared command, and without the key the same fetch fails or uses the default transport.
- identity_file/identity_agent/proxy_command values with shell metacharacters or newlines are refused at config load; the built GIT_SSH_COMMAND round-trips through shlex.split to exactly the intended tokens.
- No global Git config is edited; no key contents appear in config, doctor output, or logs.
- Old readers (096a8d74, 74549a37) load a config listing the key in ignorable_keys with a warning, and refuse it without the listing.

# Notes

Follow-on to #1929 wrkslots-provenance-only-slot-offline-probe and #1930 wrkslots-detect-worktrees-outside-wrkslots. Their issue closes are folded into this issue's landing commit.

## Resolution

[watermelon-full-preview] Landed on main in bab11f41 (tree e45b68d0, verified on the combined tree: cargo fmt --all -- --check clean, cargo check, 76 focused tests). remote_ssh_commands config key in both editions, GIT_SSH_COMMAND scoped to fetch only, shlex-quoted with metachar/newline refusal, doctor shows paths and proxy presence only. Cold security review SAFE; gate reds only waived environment families. Filed follow-on #1955 wrkslots-remote-ssh-match-by-url.
