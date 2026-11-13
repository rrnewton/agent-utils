---
title: 'gh-paced-admin-allowlist: scoped host-local allowlist for admin gh api calls'
status: closed
priority: 1
issue_type: feature
labels:
- gh-paced
created_at: 2026-11-13T00:37:00+00:00
updated_at: 2026-11-13T01:00:21+00:00
---

# Description

[watermelon-full-preview] The admin guard in gh-paced refuses every `gh api` call to an admin-only endpoint (exit 64) unless `GH_PACED_ALLOW_ADMIN_API=1`. That boolean is all-or-nothing, and a caller that cannot set environment variables for a single call (for example a runner that cannot pass env assignments) has no scoped way to admit only the few exact admin calls a task needs, such as flipping a repository's default branch or editing its rulesets. This issue adds the smallest scoped path: a host-local allowlist naming exact method, path and (where named) body fields.

## Summary

gh-paced reads an optional host-local allowlist file (default `~/.local/etc/gh-paced/admin-allow`, overridden by `$GH_PACED_ADMIN_ALLOW_FILE` or `--admin-allow-file`) whose lines name exact admin calls. An admin-only call passes the guard only on an exact method and path match (with a single `/{id}` form for `PUT`/`DELETE` on `.../rulesets`), and for `PATCH repos/OWNER/REPO` only with body fields inside the entry's `fields=` set. Every allowlisted call is logged and announced; everything else keeps today's refusal, and `GH_PACED_ALLOW_ADMIN_API=1` is unchanged.

# Acceptance Criteria

- An allowed `PATCH repos/OWNER/REPO` with only allowlisted fields passes to gh; the same `PATCH` with an extra field is refused (exit 64) before gh runs.
- A non-listed admin path (for example `DELETE repos/OWNER/REPO` or `PUT repos/OWNER/REPO/branches/main/protection`, or a path that only extends a listed one) is refused; weakening the matcher to a prefix match makes the tests fail.
- A malformed allowlist file, or one that is not mode `0600` or `0644` owned by the current user (including world-writable), refuses every admin call while non-admin calls are unaffected; no test calls the real GitHub API (the fake-gh harness is used).
- `GH_PACED_ALLOW_ADMIN_API=1` behaviour is unchanged, and an admitted allowlisted call prints one `GH-PACED ADMIN-ALLOW` line and writes an `admin-allow` audit record with method, path, fields and time.
- No real owner, repository or host names appear in shipped files; the shipped example uses placeholder names only.

# Design

- New `rs/gh-paced/src/admin_allow.rs`: line parser (`METHOD PATH [fields=a,b]`, `#` comments, no globs, `/{id}` only for `PUT`/`DELETE` on `.../rulesets`), permission and ownership checks (regular file, not a symlink, mode `0600` or `0644`, owner is the current user), body-field extraction (`-f`/`-F` keys and top-level `--input` JSON keys, fail closed when uninspectable) and the exact-match `check`.
- `Config` gains `admin_allow_path`, `admin_allow_entries` and `admin_allow_error`, resolved from the flag, then `$GH_PACED_ADMIN_ALLOW_FILE`, then the default file when it exists; a file that cannot be used records why and refuses every admin call.
- `classify_api` consults the allowlist only when `admin_only` fires and the boolean is off; `Classification.admin_allow` marks an admitted call for the wrapper, which prints the one-line notice and appends the audit record before the content guard and admission.
- Docs and help (`--admin-allow-file`, `$GH_PACED_ADMIN_ALLOW_FILE`, quickstart and user guide) plus `rs/gh-paced/admin-allow.example` with placeholder names ship the format; the live file itself is consumer data installed per host and never committed.

# Notes

[watermelon-full-preview] #1476 gh-paced-admin-allowlist, mu-ghadmin.

## Resolution

[watermelon-full-preview] Landed as 731fb376fcb9aecf12ad38cb3d2588990b161bab (`gh-paced: add scoped host-local admin allowlist`) on github/main. The scoped path is live: exact method/path (plus `fields=` for `PATCH repos/OWNER/REPO`) in the host-local file admits only that call; everything else keeps exit 64, and `GH_PACED_ALLOW_ADMIN_API=1` is unchanged. Gate passed (37 steps; the first run's two census failures did not reproduce on the fresh local pass or the re-run). The per-host live file with real entries is installed by the coordinator, not shipped here.
