---
title: 'vibe-talk-search-cli: a searchable CLI for the conversation history'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- search
- cli
created_at: 2026-11-11T18:40:47+00:00
updated_at: 2026-11-11T18:40:47+00:00
---

# Description

[watermelon-full-preview] The owner asks (reply 397): "Do we have a vibetalk CLI that agents can use? I am wondering if vibetalk could be the fast, searchable reference for the historical human<>agent conversation. Like when I ask the agent to find something and link the prior messages, it should be able to use the vibetalk CLI to search and return message hyperlinks."

There is no search CLI today. The `vibe-talk` binary offers `pair-link`, `recap`, and `cache-stats` only (`vibe-talk/src/main.rs`). Server-side search exists (#605, `GET /api/v1/channels/{id}/search` and `GET /api/v1/search`) and the MCP surface has `find_message`/`read_message`, but an agent at a shell has no way to search the conversation and get linkable hits. With a READ-scope token the server search currently answers `state: not_searched, reason: read_scope` (tested on devbig014), so read-only agents cannot search at all even though search is a read. The message cache holds a recent window, so any search covers only that window until the retention/backfill decision lands.

# Design

- New subcommand `vibe-talk search "<query>" [--channel C] [--since T] [--limit N] [--json]` against the running server with a token from config or a token file, following the `recap` subcommand's shape (config resolution, READ token, `recap::run`-style client). Without `--channel` it searches every channel (`GET /api/v1/search`); with `--channel` it searches one (`GET /api/v1/channels/{id}/search`). `--since` maps to the query language's `after:` term (or the endpoint's `below` bound where that is the honest mapping), `--limit` maps to the endpoint `limit` (default 50, max 200), and `--json` prints the structured `SearchResponse` hits. The default output is one line per hit: ET time, author, snippet, Slack permalink (`upstream_url`), and the vibe-talk deep link, newest first, matching the server's order.
- Let READ scope search. Before removing the `caller != Scope::Write` refusal in `server_search::search`, audit the whole search path for write side effects: `Sink::search` in `vibe-talk/src/cache/search.rs`, the `prepare` step (`ops::stamp`, noise marking, `render::fill_page`), and any cache, hue, thread-name, or mark writes. Search is a read; any side effect found is split out (kept on the write-scope path only, or made read-safe) so a READ token gets real hits. The `read_scope` reason and its docs (`contract::SearchScope`, `http::api` search handlers, `redact_for_read_scope` interaction) are updated to match. A focused test proves a READ-scope caller gets hits and a WRITE-scope caller is unchanged. A READ token can see everything a READ token can already read through the channel endpoints, so search widens no access (coordinator note, 2026-11-11).
- Coverage honesty: the CLI surfaces the scope lines, so "not in the cached window" is visible rather than silent: channels that were `not_searched` print their reason, channels that were `searched` print how far back they reached (`searched_back_to_ms`/`held_from_ms`, `whole`, `expanding`), and the default output ends with a coverage note when any channel is partial, capped, or still expanding. Auth failures (401/403, missing token) and unreachable-server failures print a clear one-line diagnostic to stderr and exit non-zero, never an empty success.
- Skill text: the agentctl-chat skill and the coordinator skill gain: "find where we discussed X" or "link the prior messages about Y" means run `vibe-talk search` and link the hits (Slack permalink plus vibe-talk deep link), and say when results are limited to the cached window.

# Acceptance Criteria

- `vibe-talk search "<query>"` against a running server prints one line per hit (ET time, author, snippet, Slack permalink, vibe-talk deep link), newest first; `--channel`, `--since`, and `--limit` narrow as named; `--json` prints structured hits carrying the same fields.
- A READ-scope token gets real search hits (no `read_scope` refusal); the search path performs no write side effects for a READ caller, and any side effect found in the audit is split out with a test naming it.
- A query with no hits in the cached window says so and names the window (how far back each channel was searched); a `not_searched` channel names its reason; an auth or unreachable-server failure is a clear stderr diagnostic with a non-zero exit.
- The agentctl-chat and coordinator skills state the search-and-link rule and the cached-window caveat.
- No test depends on wall-clock budgets (owner rule 346); the gate passes at normal tier.

# Notes

Owner request, reply 397 (via coordinator coordb, lane mu-vtsearch). Liaison findings: no search CLI; server search #605 exists; MCP `find_message`/`read_message` exists; READ-scope search refused as `read_scope` on devbig014. NOT in scope: full-history backfill and retention (the owner's open decision from reply 363). Until that decision lands, search covers only the cached window, and the CLI and skills say so.

Owner reply 407 (2026-11-11): "Read should cover search, shouldn't it?" Yes — slice 1 keeps the READ-scope split. Note for the record: `ops::may_serve`'s read-token rule is a #384 milestone limit (a served read's refresh fills the cache under the caller's grant), while search needs no refresh, so nothing about that limit blocks READ-scope search.

Owner reply 409 (2026-11-11): the CLI should be a multi-subcommand client exposing the same surface as the MCP server, so any agent can use it by MCP or CLI. Widened scope, same number, two slices:

- Slice 1 (today): `vibe-talk search` as designed above, `search` added to the MCP tool manifest (`mcp/mod.rs`, the same source as `tools/list` and `/api/v1/agent-tools`) with an MCP handler, and READ-scope search. Own path-selected gate and landing.
- Slice 2: a `vibe-talk` client CLI with one subcommand per MCP tool, GENERATED from the `mcp/mod.rs` tool manifest so CLI and MCP cannot drift: same argument schema, same read/write split (write tools such as `post_reply` and `mark_read` need the write token; posting asks for confirmation unless `--yes`), every subcommand takes `--json` and prints Slack and vibe-talk links where a message is involved. One test asserts every manifest tool has a subcommand and vice versa. Own path-selected gate and landing.

Resolution (mu-vtsearch, 2026-11-12): landed on main in two gated slices. Slice 1, 733b8eeb: `vibe-talk search` (READ token from config or `--token-file`, deep links on the configured public base, cached-window coverage lines, clear auth errors), `search` in the MCP manifest with its handler, and READ-scope server search — the audit found one write in the path (rendering recorded paren tags in the shared heading record), split out as `render::fill_readonly` for READ callers with the write path byte-identical; `scope_first` now proves a read session including search leaves the cache unchanged. Slice 2, 6be84aa5: generated subcommands for every offered MCP tool from the same manifest, write-token split by `mutates`, confirmation unless `--yes`, `--json` everywhere, drift test. Skills (agentctl-chat, coordinator) and README teach search-and-link with the cached-window caveat. Not verified live: no production deploy from this lane (the coordinator deploys the server change); the CLI was smoke-tested against a local fake-discord server (hits rendering needs a filled cache, which the fake backend's timeline limits prevented — server-side hits are covered by the integration suites). Full-history backfill/retention remains the owner's open decision (reply 363); until then search covers only the cached window and says so.
