# Deep links for the one-page app — #462 webapp-deep-links

[muse watermelon] Design note written before the implementation, 2026-10-23. Landed with
the code; the user-facing scheme is also in vibe-talk's README.

## The question

The owner: how linkable are the views and the messages? The app is one page served at `/`
(`/voice` already 302s there keeping the query, #302 root-is-the-app), its address never
changes, and a message can be linked only in its upstream service ("Open in …", #409).
Wanted: arguments to the single page that select the view; a link that opens the app in a
view with one message highlighted; "Copy vibe-talk link" beside the upstream link; and an
answer on durability — must the link name this server, or can any server on the same
upstream resolve it after a reboot?

## The scheme

Query parameters on `/` (never the hash: the fragment carries a pairing code and is
stripped before any request, #369):

    /?c=<channel>&v=<view>&t=<thread>&m=<provider>:<channel>:<message>[:<thread>]

- `c` — the channel id, as the channel picker holds it.
- `v` — `main`, `all`, `threads` (the thread list), `thread` (one thread across the
  window), `columns` (two columns). Absent: the reader's own restored view.
- `t` — the thread id, for `v=thread` and for the right column of `v=columns`.
- `m` — a message to focus: provider, channel, message, and the thread it is in when it
  is a reply, each percent-encoded and joined with `:`. When `m` names a channel it is
  that channel that opens, whatever `c` says.

`v=voice` is recognised and never honoured by joining a call: a link must not start a
voice session. It lands on the channel instead and says so. Unknown or malformed values
are ignored one by one and said in the status line; a link is never allowed to break the
open.

The page writes the address as it goes: every new place (#423's places, noted in
`notePlace`) replaces the query in step, `history.replaceState` with the entry's own
state kept, so no browser entries are added or taken and #304's Back stack is untouched —
its pushed entries carry no URL of their own and inherit the current one. The focused
message stays in the address until the reader moves to another place, so the address bar
itself is copyable; selection alone does not rewrite it. On load the address wins over
the device's restored state (#189): a link says where to open.

Opening a message link: open its channel and view, then find the message by the reply
arrow's own walk (`seekMessage`): on screen, or in All, or fetched page by page further
back (bounded, as that walk is), landing with the same flash and selection
(`revealRow`). If the walk ends without it, the status line says which of the walk's
answers it was — hidden by a filter, not in this channel's history, further back than
was walked. A message in a thread opens that thread; a message whose thread cannot be
shown falls back to All, which shows every message.

"Copy vibe-talk link" stands in the ⋯ menu under "Copy <service> link", always offered
(it needs no upstream address): for a reply it names the thread view, for any other
message Main (where a root stands), with `m` naming the message.

## Durability

A link names no server but the host it is opened on, and no server-local row id. Its
parts are the upstream's own identity as this server canonically derives it:

- Discord: channel and message snowflakes verbatim.
- Slack: the conversation id verbatim; the message `ts` losslessly encoded as the
  snowflake-shaped id the whole server speaks (`slack/ids.rs` — a pure function of the
  `ts`, decoded back exactly).
- Google Chat through a bridge: the Chat resource names the bridge reports.

So any vibe-talk server connected to the same upstream derives the same ids and resolves
the same link, and a reboot changes nothing: no store, cursor or session is named. The
provider segment is the service's name ("Slack", "Discord", …), matched
case-insensitively; it disambiguates id shapes that could collide across services and
is advisory — a resolving page whose provider is named differently still tries the
channel and message. What can break a link: the channel no longer configured on the
server it is opened on, the message deleted or beyond the fetch walk, or a future change
to the id derivation itself. Thread ids are their root message's id (Discord threads are
channels, named by their own snowflake as served), so they are as durable as message ids.

## Privacy

A link carries ids only — provider, channel, message, thread — never message text,
author names, excerpts or credentials; the token stays where it has always been, in
storage and the Authorization header, and the pairing fragment is still stripped before
anything can copy the address. Whoever holds a link still needs the deployment's token
to read anything through it; the ids are the same ones the upstream's own permalinks
already publish.
