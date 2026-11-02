---
title: 'reference-links: draw configured reference ids in message text as links'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-01T21:37:23.635560244+00:00
updated_at: 2026-11-02T04:37:23.003835702+00:00
closed_at: 2026-11-02T04:37:23.003835552+00:00
---

# Description

[opus 5.5] The owner (2026-11-01): reference ids in message text, such as a code review's `D123456`, are drawn as plain text where they should be links. That includes the owner's own messages, which name the id without the full address.

Wanted: a configurable list under `[render] reference_links`. Each entry is a regex `pattern` and a `url` template naming its groups (`$1`, `${name}`). The server-side Markdown render applies them to the words of a message: never inside code spans or blocks, never inside an existing link's text, and never inside an address. Bad rules are refused when the configuration loads: a regex that does not compile, a pattern matching the empty string, a non-http(s) url, or a group the pattern has not got. The public repository names example domains only; a deployment's real patterns live in its own app.toml. The search index's version changes with the rules.

Also check whether Slack delivers such ids as link markup (`<url|D…>`) that the reader drops.

Slack, as checked by the coordinator (2026-11-01): most stored mentions are already a Markdown link (`[D…](address)`, written by an agent or converted by Slack). The reader keeps those as links, and they are not linked a second time. An id the owner types is stored as plain text, and only Slack's own client links it when showing it: that is the case these rules cover.

Related: #657 thread-reference-links (Slack thread links as in-app jumps), a different kind of reference.
