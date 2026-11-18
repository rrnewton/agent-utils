---
title: 'vibe-talk-tts-skip-timestamps-and-auto-link-ids: skip timestamp citations in speech, auto-link minibeads ids'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T01:15:00.000000000+00:00
updated_at: 2026-11-18T01:15:00.000000000+00:00
---

# Description

Owner request (voice-dictated): read-aloud preprocessing should also
omit the parenthesized timestamp links the page renders, e.g.
"(13:33 ↩)". Agents cite minibeads issues and forget to hyperlink
them; the summarize agent is probably not subject to the same
hyperlink rules as normal agents, so making the processing automatic
in the vibe-talk layer is simpler. Later these should be configurable
and extensible patterns that match a pattern and map it to a
constructed URL. The owner asks: did we make any general
infrastructure for that purpose, and where does the configuration
state live? It is not exposed in the Settings interface.

# Design

What exists today, in plain words: yes, the general infrastructure
exists. Message rendering already has configurable reference-link
rules: a list of pattern-to-URL rules in the app.toml file, under
[render], key "reference_links". Each rule is a regular expression
"pattern" and a "url" template that names capture groups as $1 or
${name} (what is captured is percent-encoded). The existing rules in
production map diff, task and paste ids (the D/T/P rules) to their
review and task pages. A sibling key, "issue_url_template" (an
overlay), says where ":issue:NNNN" short references point, using one
{n} placeholder. Both are file-only configuration: they are read from
app.toml at startup, checked when the configuration loads (a rule
that does not compile, matches empty text, names a missing group, or
is not http(s) with its own host refuses to start), and are not shown
or editable in the Settings interface. The summarize and Discuss path
renders through the same rendering pass as channel messages
(render_named_catchup into the shared adjust pass that applies
reference_links and the short-reference pass), so a configured rule
applies to Discuss output automatically, whatever the model writes.

Design only, for owner review (no code in this issue):

- Settings surface: show reference_links and issue_url_template in
  Settings, read from the same app.toml values today. Editing would
  write back to that file (server-side, owner scope only), validate
  with the same load-time checks before saving, and apply without a
  restart by recompiling the rule set. State stays in app.toml; the
  Settings page is a view and editor over it, not a second store.
- Per-project rule sets: a rule set is just rules in that file, so a
  project ships its own list. For minibeads, one rule maps the
  project prefix plus number to the issue file on the main branch of
  its GitHub repository. For a deployment operating on another code
  host, the same pattern shape maps to that host's file view on its
  primary branch instead; only the url template changes.
- Migration: the existing D/T/P rules already live in this exact
  form, so nothing migrates in format; they would simply appear in
  the Settings list alongside any project rules, editable in place.

This change: speech preprocessing drops timestamp citation links,
Discuss linking is verified and tested end to end, and the minibeads
rule is proven as configuration (a test with that rule in a test
config), not hard-coded. The exact app.toml snippet is handed to the
coordinator, who installs it on the live instance at deploy.

# Acceptance Criteria

- for_speech drops "[(HH:MM ↩)](...)", "[(HH:MM ↗)](...)" and the
  plain "(HH:MM ↩)" / "(HH:MM ↗)" forms, in Discuss output and in
  message reads, with tests in speakable.rs; ordinary times and
  prose are unchanged.
- A test proves Discuss-shaped output rendered through the catchup
  path links a configured reference id, using a minibeads-shaped
  rule in a test config; no project rule is hard-coded in vibe-talk.
- Screenshot matrix on a test instance: Discuss output showing a
  linked minibeads-shaped id, phone and desktop, light and dark.

# Notes

Owner-priority, deployed on landing. The minibeads rule itself is
configuration for the coordinator to install at deploy; this change
proves the rule shape works and hands over the exact snippet.

## Resolution
