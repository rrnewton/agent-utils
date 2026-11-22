---
title: 'vibe-talk-unified-cache-read-cli: one cache-only CLI for message search, owner prompts and thread listing'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- cli
created_at: 2026-11-22T09:55:00-05:00
updated_at: 2026-11-22T09:55:00-05:00
---

# Description

The owner wants the read CLIs unified. Today the prompts command lists the owner messages with no keyword filter, and the search command goes through the running server. The owner asked for message-granularity keyword search, real time ranges, and easy thread enumeration.

# Design

A cache-only reader (no upstream chat calls, never the live server): search with AND of terms, quoted phrases, an any-of mode, case-insensitive, printing whole messages or a brief line with channel, thread (long id, slug, short id) and Eastern time, plus JSON; a mine filter so prompts becomes search with that filter and no terms; time by since (end defaults to now), until, and a from..to range, accepting a date or date and time in Eastern; a threads command listing threads active in a window (ids, slug, short id, counts, last activity) and a thread command listing or searching one thread; keep the channel filter. Existing commands keep working as aliases.

Refined in place by the lane (built as specified here):

- New module `vibe-talk/src/read_cli.rs` is the one reader. It opens the cache file read-only (`SQLITE_OPEN_READ_ONLY`) and short-ref files from disk; it names no server address, token or provider. The old server flags (`--server`, `--token-file`, `--base-url`) are refused at parse with a cache-only message, so no path drifts back to the running server. The former server search stays available as `search-server` beside it; `search` is now the cache-only command.
- `search [TERM...]`: bare terms AND, `--any` OR, `"quoted phrase"` contiguous, all case-insensitive folded substrings of author + text over the WHOLE message (terms on different lines of one message hit together). Hits newest first, whole message or `--brief` one-liner, with channel, ET time, and thread as Slack long id + slug (a root's filed `display_name`, held to slug shape) + `:thread:N` short id (from `[render] short_refs_files` or `--short-refs`). `--json` structured hits. `--limit` default 200.
- `--mine`: only the configured owner's messages. `prompts` is unchanged and `search --mine` with no terms (full text) renders through `prompts` itself, line-for-line equal; a test asserts the equality.
- Time: `--since` (end defaults to now), `--until`, `--range FROM..TO`, `--hours` (kept, default window 24h); a date or `"date HH:MM"` in ET. Windows are `[since, until)`; a date-only `--until`/`TO` covers its whole day. Conflicting windows refuse.
- `threads`: threads with a cached message in the window, newest activity first — long id, slug, short id, in-window message count, last activity in ET; `--channel`, `--limit`, `--json`.
- `thread <long-id|slug|:thread:N|N> [TERM...]`: one thread by any of its names (ambiguous short ids/slugs refuse, `--channel` settles), messages oldest first, whole cached thread unless time flags narrow it; terms search inside it with the same AND/any/phrase rules.
- Config discovery, channel matching (`--channel` by id/label/alias/#name), owner ids and cache-path resolution reuse `prompts` exactly (lenient, tokenless). Existing `prompts` flags and output are untouched.

# Acceptance Criteria

- Tests against a fixture cache for AND/phrase/any matching at message granularity, the mine filter, since/until/range boundaries in Eastern time, threads listing and thread search; no network or server access in any path (asserted).
- Built: `vibe-talk/tests/read_cli.rs` (in the `integration` binary) covers AND across lines, phrase contiguity, OR, case folding, `prompts == search --mine`, ET since/until/range boundaries (inclusive since, exclusive until instant, whole-day date-only until), channel filter, JSON thread triple (long id, slug, `:thread:N`), threads listing order/counts, thread read by long id/slug/short id/bare number, in-thread search, and parse refusal of the server flags against a synthetic fixture cache with a tokenless config.

# Notes

Filed by the coordinator from an owner request; the lane refines this file.
