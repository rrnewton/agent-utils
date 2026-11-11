# vt-unread-recap: design note

Date: 2026-10-31. Issue: #717 vt-unread-recap. Status: implemented in this change; this note is
the working design, superseded by the README section and `vibe-talk recap --help` once landed.

## Problem

The owner marks vibe-talk messages Done as reading progress. After 12–24 h away there are too
many updates, many reporting transient statuses. An agent asked for a recap over a window must
see what the owner has not read, and must not repeat what the owner already read.

## Semantics (owner-confirmed, verified in code)

- **Read = Done/archived**: the per-message dismissal row behind
  `POST /api/v1/channels/{id}/dismiss` and `/restore`
  ([api::dismiss](../../../vibe-talk/src/http/api.rs), [ops::dismiss](../../../vibe-talk/src/ops.rs),
  `StateStore::dismissals/dismiss/restore` in [store/mod.rs](../../../vibe-talk/src/store/mod.rs)).
  A message is DONE exactly when its id is in the channel's dismissal set, plus the owner's own
  messages whose first read state was decided Done at first sight
  ([own_read::settle](../../../vibe-talk/src/own_read.rs), surfaced by `ops::dismissed_within`).
- **Thread replies**: Done is per message, never per thread. Dismissing a thread root does not
  dismiss its replies, and dismissing a reply does not dismiss the root. A reply's mark is stored
  under its parent channel's id with the reply's own message id
  (`dismissals (channel_id, message_id)`), which is why Main's timeline can report
  `reply_dismissals` separately from the root rows' `dismissed`
  ([ops::reply_dismissals](../../../vibe-talk/src/ops.rs), `#222 unread-replies`). The recap
  therefore tags every root and every reply independently from the same dismissal set.
- The per-channel read mark (`store ReadMark.last_read`, moved by
  `POST /api/v1/channels/{id}/read`) is secondary: shown in the header, never used to tag a
  message. A message after the mark can already be Done, and one before it can be UNREAD after a
  restore.
- Noise-rule messages (`Message.noise`, `#196 auto-read-noise`) are the owner's standing
  auto-read placeholders: excluded from the listing and counted in the header, as `/digest` and
  the to-do view already do.

## Why a new server endpoint

Existing READ routes cannot answer this in one honest shape:

- `GET /channels/{id}/page?since=&until=` reads the channel's own messages only
  (`ReadScope::Main`; the emulated range read refuses other scopes), so thread replies are
  missing.
- `GET /channels/{id}/timeline?view=flat` interleaves replies but pages backwards from now with
  opaque cursors and no time window; composing a 24 h window client-side takes an unbounded
  number of round trips and still cannot say which replies of a thread predate the window.
- Neither returns the read mark; that takes a second call to `GET /api/v1/inbox`.

So: `GET /api/v1/channels/{channel_id}/recap?since_ms=<ms>&until_ms=<ms>` (also `hours=<1..168>`
standing alone), READ scope like every other history read. The server walks the flat timeline
backwards inside the window (bounded: at most 50 pages / 2,000 in-window messages, `truncated`
says when the bound cut the window), drops noise (counted), tags each message DONE/UNREAD from
the dismissal set, groups by thread oldest first, and for a thread whose root predates the
window fetches that thread's opening: the root plus its first two replies, tagged CONTEXT, with
the count of earlier replies hidden between the opening and the window. The per-channel read
mark rides in the same answer. No writes; a read-scope call decides nothing durably
(`own_read::settle` only answers *as if* for read scope).

## CLI: `vibe-talk recap`

Same discovery model as `vibe-talk pair-link`: `Config::discover` (`--config`,
`--config-overlay`, `VIBE_TALK_CONFIG`, environment), the server reached on the configuration's
bind address (unspecified → loopback; `--server URL` overrides for a test server), the READ
token (`VIBE_TALK_READ_TOKEN` wins over the file, as the server reads it), never printed.

- `--hours N` (1..=168) or `--since <permalink|message id|time>`; neither means `--hours 24`;
  both together are refused. A permalink is recognised by `message_links::recognize` and also
  selects its channel when `--channel` is absent. A bare snowflake uses its embedded creation
  instant; any other message id is looked up through the server. A time is an RFC 3339 instant,
  or `YYYY-MM-DD[ HH:MM]` read in America/New_York.
- `--channel <id|label|alias>`: default every channel `GET /api/v1/channels` returns for this
  token, in list order. Documented deviation from "the configured main one": there is no single
  configured main channel; the channel list is the token's whole world.
- Output markdown, oldest first, grouped by thread. Header: window in ET (America/New_York,
  labelled ET), channel, read-mark time, DONE/NOT DONE/noise counts. Each message: permalink
  (upstream URL when the provider formed one, else a short id), ET time, sender, DONE or UNREAD,
  text. A pre-window thread opens with its start line and hidden count, its root, then the
  bounded CONTEXT opening. Footer instructs the reading agent: do not repeat DONE content
  unless its status changed later in the window; count superseded transient statuses only in
  their final state.
- `--max-chars` (default 24000, 0 = no limit) bounds the markdown only: DONE messages shrink to
  one line first, then to their header line; UNREAD text is never cut, so the output may exceed
  the budget when UNREAD alone does — and says so.
- `--json` prints the same data structured (`{channels: [RecapResponse]}`), unshrunk.

## Skill: `vibe-talk-recap`

`skills/vibe-talk-recap/SKILL.md`: when asked for a recap since a time or event, run the CLI and
report by topic — (1) what changed in the window that the owner has not read, (2) what is still
in flight, (3) decisions awaiting the owner — skipping read material unless its status changed
afterwards, linking permalinks instead of repeating text.

## Bounds and refusals

- `until <= since`, or `hours` outside 1..=168: `invalid_range` / `invalid_hours`, matching
  `/page`. The span itself is unbounded — `--since` an old event must still answer — and the
  walk is bounded instead (50 pages / 2,000 in-window messages, `truncated` says when cut).
- Unknown channel: `unknown_channel`, as every channel route.
- No store configured: DONE tags are all UNREAD and the read mark is absent rather than an
  error — the recap is still the window's history; the header says no read mark is held. (The
  to-do view refuses there because filtering *is* its answer; a recap's answer is the window,
  with read state as annotation. `dismissals` on a disabled store errors, so the endpoint
  treats `Unavailable` as an empty set.)

## Out of scope

No page changes (`web/`, `voice.js`, `voice.html` frozen by #624 voice-page-modularity), no MCP
tool, no write of any kind from the CLI.
