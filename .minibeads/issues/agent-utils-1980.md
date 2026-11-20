---
title: 'vibe-talk-docs-private-github-load: Documents reader fails on a private GitHub link with a bare error'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-20T13:45:00-05:00
updated_at: 2026-11-20T14:39:00-05:00
closed_at: 2026-11-20T14:39:00-05:00
---

# Description

[watermelon-full-preview] Owner bug 256 (2026-11-20 1:31 PM ET): a GitHub blob link to a file in a private repository attempts to load in vibe-talk's Documents reader but says only "could not load document". The linked file exists on the branch named in the link. Candidates: how the server fetches GitHub blob links (raw URL rewrite, auth for private repositories, which credential or fetcher it uses, proxy), path handling, or the error mapping.

# Design

- If the server can fetch private-repository content with a credential it is already allowed to use (the `[documents]` GitHub token file/command), make it work and prove it against a stub fetcher that refuses unauthenticated requests.
- In all cases, a failure shows a clear, specific reason (private repository / no access, not found, rate limit, network) plus an "Open on GitHub" link, never a bare "could not load document".
- Reproduce and test on a local server with a fixture or stub fetcher only; never the live server. End-to-end through the real router.

# Acceptance Criteria

- A stub GitHub raw host that answers 404 without a credential and 200 with the configured credential serves the document through the real `GithubFetcher` and through `POST /api/v1/documents/fetch` plus `GET /api/v1/documents/{key}`.
- A private/no-access answer (401/403, or a 404 whose tree lookup is also refused) is reported as no access to the named repository, not as "moved or deleted"; a file genuinely absent from a reachable tree is reported as not found; a rate limit and a network failure are named as such.
- The reader, on a failed load, shows the specific reason and an "Open on GitHub" link to the document's own address (safe http(s) only), in place of the bare "Could not load the document."
- Existing documents tests still pass; the reader failure is covered at phone and desktop-relevant layers without calling GitHub or the live server.

# Notes

[watermelon-full-preview] Root cause: the fetcher (`GithubFetcher::get` in `vibe-talk/src/documents.rs`) rewrites a blob link to `raw.githubusercontent.com/<owner>/<repo>/<ref>/<path>` and sends the configured `[documents]` credential as a bearer token when one is configured. For a private repository with no usable credential GitHub answers 404 (raw) and 404 (tree); the old code swallowed the tree failure (`moved_path(...).await` via `.ok()?`) and reported "not found at … (moved or deleted)", then the reader (`open()` in `web/src/view/documents/reader.ts`) discarded the server's detail entirely and showed only "Could not load the document." with no way out. So a private link, a missing file and a network failure all reached the owner as the same bare sentence.

Fix: the tree lookup's failure is now the fetch's failure, classified by status (401/403 = no access, 404 with no credential = no access / private cannot be fetched, 429 or rate-limit 403 = GitHub rate limit, other = GitHub's status, transport = "could not reach GitHub"); only a tree that answered may say "not found … (moved or deleted)". The reader keeps the document address from any answer that landed, shows the specific reason in the body and status, and adds an "Open on GitHub" link (safe http(s) only, taken from the server row, never constructed by the page); a failed list card does the same. Credentialed private fetch is proven against a stub that 404s without the bearer and 200s with it, through the fetcher and through the real router; a local real-binary walk with a stub raw host loads the private file with `github_token_file` set and returns a specific 502 without one.

## Resolution

[watermelon-full-preview] Landed 2026-11-20 on main as 63a58bb4 + 8b00a4d6 (replayed tip of 6a5b0548, `--replay-base 5b0c5123`, no file overlap) under coordb's per-landing waiver for this landing only; coordb deploys on landing. Evidence: `cargo test --locked --test integration -- documents::` 23 passed on 6a5b0548 (private stub 404-without/200-with the configured credential, through the real router; rate limit named); documents JS tests, the page shell test and `make typecheck` pass; a dynamic walk of the real binary (fake chat, stub GitHub raw host) loaded the private file with `github_token_file` set and returned a specific 502 without one. Gate on 6a5b0548 (205 steps) had no non-browser failures; its 149 browser/shots/pwa reds were the missing Chromium libraries on the gating host. On devbig030 at the same tree (BUILD.txt verified), `tests/documents_browser.py` passed rc=0 including the private-reader failure state (specific no-access reason, "Open on GitHub" link, never the bare sentence) and scenes 73-76 passed; scene 77-composer-keyboard-open failed on laptop-1280 with "the composer holds the focus", a channel-composer state outside this change, noted as a pre-existing scene issue. Follow-up worth doing: save a screenshot of the private-repo failure state (the browser test asserts it but does not photograph it).
