---
title: 'documents-relative-links: render document links and open GitHub documents in-app'
status: open
priority: 1
issue_type: task
assignee: mu-docreader
created_at: 2026-11-08T18:02:09.000000000+00:00
updated_at: 2026-11-08T18:02:09.000000000+00:00
---

# Description

Owner (thread 214, reply 235), reading a changelog that is an index into other documents: "the whole point of that doc is to be an index into other documents and it highlights a problem with our markdown rendering. We don't seem to render the hyperlinks at all that link the other documents. And furthermore, since those are just GitHub markdown links we should automatically reinterpret them as intra-app vibe talk links. So navigation should be convenient including the back button."

Measured on that document: 417 of its 419 markdown links are relative (e.g. `](reports/2026-11-08-buck-only-builds-handoff.md)`, `](CHANGELOG-PROTOCOL.md)`). The reader's renderer (`view/documents/render.ts`, `allowedLinkHref`) renders only `http(s)://`, `#/` and `docs:` hrefs as links; relative and protocol-relative addresses render as plain text, so an index document is dead text. Absolute github.com blob links do render, but as ordinary external anchors: tapping one leaves the app instead of opening the in-app reader the way #1073 documents-in-app-links does for message links.

# Design

- Render every link. The renderer needs the open document's context (its key, hence owner/repo/ref/directory for `github:owner/repo@ref:path` keys): thread a link base into `renderDocumentMarkdown` (or resolve in a reader post-pass over `#doc-body` anchors, the way `wireMessageLinks` marks message anchors). Resolve relative references against the document's directory with safe normalization (`./`, `../` never escaping the repository root; a leading `/` is repository-root-relative on GitHub), and split off `#anchors` (an anchor naming a heading of the target scrolls there after opening; a bare `#heading` stays in the open document).
- A resolved link that names a GitHub document (the server's own classification: `github.com/owner/repo/blob/ref/path` or raw form, `.md`/`.markdown` as markdown, other files as documents too) becomes an in-app document link: the canonical server key (`documents.rs` minting rules, so client and server agree), a plain tap opens it in the reader via the same `data-doc` marking + wiring #1073 uses, and a modifier-click (ctrl/meta/shift/alt/middle) follows the real GitHub address externally. Non-document links (other hosts, mailto and friends) stay external anchors under the existing allowlist discipline; unsafe schemes still render as text.
- The server must admit a document that no message has mentioned: today `GET /api/v1/documents/{key}` and `POST /api/v1/documents/fetch` both 404 `unknown_document` for keys outside the index, so most index-document targets cannot open at all. Add the smallest admission (fetch-by-key registering the entry, or fetch-by-URL canonicalized server-side) under the existing write scope and bounded-cache rules; the reader's existing fetch-on-open then serves. A target that does not exist upstream fails the way a failed fetch fails today, on the reader, with Back intact (#1082 documents-back-chain).
- Modules: `vibe-talk/web/src/view/documents/render.ts` + `reader.ts` (link resolution/marking and click wiring), possibly `view/shell/navigation.ts` if the marking is shared with `wireMessageLinks` (coordinate through coordb; mu-layout may be in that file), `vibe-talk/src/documents.rs` + `vibe-talk/src/http/api.rs` for admission. The documents list module is not touched.

# Acceptance Criteria

- Unit checks: relative resolution table — sibling, `./`, nested `../`, root-absolute, escaping-above-root refused, anchors split and applied, percent-escapes preserved, non-GitHub and unsafe targets unchanged in behaviour; key minting matches the server's for blob and raw forms.
- Browser check on a phone width (393x852, real touch, real server API): open an index-style document whose links are relative; every link renders as a link; a tap on a relative document link opens that document in the reader (fetching it first if never mentioned in chat); a modifier-click opens the GitHub page in a new tab; an external non-document link still leaves the app.
- The renderer's safety suite stays green: no document-supplied markup, scheme, or attribute reaches the page (existing inert-rendering tests plus new relative-link cases).

# Notes

[watermelon-full-preview] mu-docreader, thread 214. Sibling: #1082 documents-back-chain (the chain these opens travel).
