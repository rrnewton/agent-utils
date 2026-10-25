# Evidence 3: Discord and Google Chat, from their published API definitions

2026-10-25, `#521 server-side-search`. Not exercised live: no Discord bot token or Google Chat user
token was used in this pilot. Sources, fetched read-only:

- Discord: `discord/discord-api-docs` at `6128d99116af` (2026-10-23),
  `developers/resources/message.mdx` and `developers/change-log.mdx`.
- Google Chat: `googleapis/googleapis` at `dff1cd0c7ff8` (2026-10-24),
  `google/chat/v1/message.proto` and `chat_service.proto`.

## Discord

**History.** `GET /channels/{id}/messages`: `limit` 1–100 (default 50), and exactly one of
`before`, `after`, `around` (a message id). Newest first. A thread is a channel of its own, so an
exhaustive read of a text channel is its history plus every thread's (active and archived thread
listings, which vibe-talk already walks in `discord/threads.rs`).

**Search, for bots, since 2026-03-19.** `GET /guilds/{guild.id}/messages/search` ("Search Guild
Messages"), documented for bots on 2026-03-19. Requires `READ_MESSAGE_HISTORY` and the
`MESSAGE_CONTENT` privileged intent. Parameters that matter here:

| Parameter | Meaning |
|---|---|
| `content` | text to match, ≤ 1024 characters; `slop` (default 2, ≤ 100) is "max number of words to skip between matching tokens", so matching is by **token**, phrase-ish |
| `channel_id[]` | ≤ 500 channels |
| `author_id[]`, `author_type[]` (`user`, `bot`, `webhook`, negatable) | who |
| `min_id`, `max_id` | snowflake bounds: a time window |
| `has[]` (`link`, `file`, `image`, …, negatable), `pinned`, `mentions[]` | structural filters |
| `limit` 1–25, `offset` ≤ 9975 | so at most 10,000 results per query window |
| `sort_by` `timestamp` (default) or `relevance`; `sort_order` | |

Caveats in the definition itself: a guild not yet indexed answers **202** with
`"Index not yet available. Try again later"` and `retry_after`; "search may return slightly fewer
results than the limit specified when messages have not been accessed for a long time. Clients
should not rely on the length of the `messages` array to paginate"; and `total_results` "may not be
accurate" while messages are being created or deleted. No boolean `OR` in `content`. As with
Slack: candidates, never proof of absence.

## Google Chat

**List.** `spaces.messages.list`: `page_size` ≤ 1000 (default 25); `filter` on `create_time`
(`>`/`<`, RFC 3339, `AND` for an interval) and one `thread.name`; `order_by` `ASC`/`DESC` on
create time; `show_deleted`. **No text filter.** An exhaustive, time-windowed read of a space,
thread replies included (a space's messages list is not split by thread). The `#208
persistent-cache` issue records a whole-space read at about 11 s through vibe-talk's path.

**Search.** `spaces.messages.search` (`POST /v1/{parent=spaces/*}/messages:search`, parent must be
`spaces/-`, narrowed with `space.name = "spaces/…"` in the filter). **User authentication only**
(`chat.messages.readonly` or `chat.messages`), never app authentication. The filter takes keywords
and quoted phrases (implicitly ANDed: `"things to do" "urgent"`), `create_time` `<`/`>=` as one
interval, `sender.name =` (OR only among senders), `attachment:*`, `has_link()`, `is_unread()`,
mentions. **No OR between keywords**, no exclusion documented. `page_size` ≤ 100; `order_by`
`create_time desc` (default) or `relevance` (developer preview); descending only. Query ≤ 1000
characters.

A vendor CLI on the pilot host also offers a Google Chat search backed by a private index
(exact 1-gram keywords, optional stemming or vector search), whose own help says keyword-less
scans "are not guaranteed exhaustive" and points to the list read for exhaustive reads.

## Summary across providers

| | Exhaustive read (proof) | Text search upstream (hints) | OR | NOT | Phrase | Infix substring | Author by name | Time window |
|---|---|---|---|---|---|---|---|---|
| Slack, Real-Time Search (measured) | `*` listing, 10/call, 200/query, replies included; or history 20/call + replies per thread | yes, user or app-user token | no | `-t`, `NOT t` | yes | no (prefix only) | no (`from:<@ID>`) | `after:` `before:` `on:` `during:`, created bounds |
| Discord bot | history 100/call + each thread | Search Guild Messages (2026-03), `MESSAGE_CONTENT` intent | no | structural only | slop | no (tokens) | `author_id` | `min_id`/`max_id` |
| Google Chat | list 1000/call, `create_time` window | `messages:search`, user auth only | no | no | yes | no (keywords) | `sender.name` | `create_time` |

No provider's text search can stand in for the page's matcher: none has OR, none matches inside a
word, none searches display names, and Slack's measurably drops whole-word matches. So upstream
search can only ever be a fast hint, re-checked here; every claim about how far back a search
looked has to rest on an exhaustive, time-windowed read.
