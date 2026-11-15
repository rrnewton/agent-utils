---
title: 'oss-documents-view: Documents view empty and GitHub markdown links do not open in the reader on the open-source deployment'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-15T17:20:00+00:00
updated_at: 2026-11-15T17:20:00+00:00
---

# Description

## Goal
The open-source vibe-talk deployment's Documents view shows no documents apart from one broken entry that will not open, and clicking a GitHub markdown link (`https://github.com/<owner>/<repo>/blob/main/ai_docs/<FILE>.md`) in a message does not open it in the Documents view. Diagnose whether this is a client bug, a server bug or a stale deploy, fix it, and make GitHub blob links to `.md` files open in the Documents reader in both the open-source build and the internal build, with browser tests at phone (~390 px) and desktop widths.

## Evidence
On current `main` the Documents feature is complete and verified: the server folds cached messages into a document index (`vibe-talk/src/documents.rs`), marks recognised document links in rendered message bodies with `data-doc` (`vibe-talk/src/render.rs`), the page opens those links in the reader (`vibe-talk/web/src/view/shell/navigation.ts`), and `vibe-talk/tests/documents_browser.py` passes against a native release build (list, fetch, reader, message-link tap at 393 px and desktop). The open-source container build on `main` is broken: `vibe-talk/Cargo.toml` depends on the sibling path dependency `../rs/claude-stream-session`, while `vibe-talk/Containerfile` and `vibe-talk/scripts/run.sh` build with the `vibe-talk/` directory as the build context, so Cargo cannot load the manifest inside the image build (reproduced by copying `vibe-talk/` alone and running `cargo metadata`). The deployed open-source instance therefore cannot be rebuilt from current `main` and predates the landed Documents work. PR #322 (container build context fix) is correct and needed; PR #326 (app-name passthrough) is correct but unrelated to Documents.

## Fix
Build the container image from the repository root so the sibling Rust path dependencies are inside the build context: update `vibe-talk/Containerfile` copy paths, `vibe-talk/scripts/run.sh` build context, the CI workflow build context, the README build command, and add a root `.dockerignore` keeping local configuration and generated artifacts out of the context. Extend `vibe-talk/tests/containerfile.rs` and `vibe-talk/scripts/test-run-sh.sh` so the build-context requirement is pinned by tests.

## Acceptance
The container build context includes the sibling Rust workspace and the image build can load the vibe-talk manifest; `vibe-talk/tests/documents_browser.py` passes (GitHub blob `.md` links in messages open the Documents reader at phone and desktop widths); screenshots of the Documents list and reader at 390 px and desktop are inspected; capped validation and the gate pass on the exact landed HEAD.
