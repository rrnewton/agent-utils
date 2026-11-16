---
title: 'vibe-talk-short-ref-link-double-render: bridge-expanded short references render as the raw URL plus label in vibe-talk'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T13:29:00.000000000+00:00
updated_at: 2026-11-16T14:05:00.000000000+00:00
closed_at: 2026-11-16T14:05:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner report (fast-track): in vibe-talk, every bridge-expanded short reference renders as ONE underlined run holding the full URL followed by the label, e.g. `https://…/p1794765511844489 :thread:102` and `https://…/agent-utils-1682.md :issue:1682`. A plain Markdown link in the same message renders correctly.

The stored body (read from the local message cache, read-only) holds the flattened history shape `[https://…/p1794765511844489 :thread:102](https://…/p1794765511844489%7C:thread:102)`: the history reader's double conversion of the bridge's `<url|:thread:102>`, without the angle brackets the #1606 repair (`repair_fallback_links`) requires. The repair only matches `<[text](href) rest>`, so this bare form passes through and comrak draws it as one link whose text is the raw URL plus label and whose address still holds `%7C`.

# Design

vibe-talk side: extend the #1606 fallback-link repair so the same exact shape is recognised without its angle brackets — a Markdown link whose text is `{url} {word}` and whose address is `{url}%7C{word}` (plus any label rest) is rewritten to `[{word}{rest}]({url})`. The exactness guards stay: the address must decode to the link text with its first space for the pipe, so a link a writer really wrote is left as written. After the repair a bridge-expanded `:thread:` is one link to the thread permalink, which the existing message-link marking recognises for the in-app thread drill; a `:issue:` (or the bridge's newer `#N` label) is one link to the issue address with the short label. Nothing inside a link is re-expanded by the short-ref pass, so each reference renders exactly once. The bridge form after #1709 (`<url|#N>`, Markdown `[:thread:NNN](url)` on the converted path) flattens to the same shape and is covered by the same repair.

# Acceptance Criteria

- A bridge-expanded `:thread:` / `:email:` / `:issue:` reference (either the `:issue:` label or the `#N` label) renders in vibe-talk as ONE anchor with the short label, never the raw URL as text, and its address holds no `%7C`.
- The history shape and the live shape of the same message render identically.
- A genuine Markdown link whose text or address merely resembles the shape is left as written.
- A screenshot e2e (sample reply with `:thread:`/`:issue:` refs, an en dash, and a >4 KB body, through the bridge's outbound conversion into a test vibe-talk instance at phone and desktop widths) shows one clean link per reference.
