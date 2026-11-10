---
title: 'documents-collapsible-sections: fold the reader by heading, with Collapse all and Expand all'
status: open
priority: 2
issue_type: feature
assignee: mu-docread
created_at: 2026-11-10T17:56:44+00:00
updated_at: 2026-11-10T17:56:44+00:00
---

# Description

[watermelon-full-preview] Owner reply 338 (2026-11-10): the Documents reader's hamburger menu offers "Expand all" even though it makes no sense in the document view today — but collapsible sections would be a nice feature, and then collapse/expand all would belong there. This is the nice-to-have half of that remark, filed on its own so the two fast-tracked fixes (#1235, #1236) never wait on it. #1236 hides the channel's "Expand all" in the document view; this issue would give the view one of its own that means something.

# Design

- Each heading in `#doc-body` folds the section under it (everything up to the next heading of the same or higher level): a tap or key on the heading collapses or expands it, with the state said accessibly (`aria-expanded`) and the heading staying on screen. Folding is view state only; it never alters the stored document.
- "Collapse all" and "Expand all" for the open document, offered where the reader's own controls live (and only there), each acting on every section at once.
- The outline pop-up (#1038) and its HERE marking keep working across folds: jumping to a heading inside a folded section unfolds the way to it first. Read-aloud is unaffected — it reads the document, not the folds.
- A document opens fully expanded. Whether folds persist per document across visits is a design question to settle before building, not a default.

# Acceptance Criteria

- Browser checks with real input at desktop and 393x852 (touch), against the real server documents API: a heading folds and unfolds its section (and nested subsections with it), Collapse all folds every section to its headings, Expand all restores the full text, and an outline jump to a folded-away heading lands on it visible.
- Keyboard: the fold control is reachable and operable without a pointer, and says its state.

# Notes

[watermelon-full-preview] mu-docread, owner reply 338 batch, piece C of three. Nice-to-have (P2): not built with the fast-tracked pieces. Siblings: #1235 documents-list-continuation, #1236 documents-reader-layout. Depends on the reader as #1236 leaves it; the outline behaviour is #1038 documents-outline-popup's.
