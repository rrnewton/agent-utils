# replication-model: what the page, the server and the provider each hold, and how it should merge

Working design, 2026-10-23. Not user documentation. `#454 replication-model`.

Code references are to `main` at `a4c9afd6` unless they name the `vt/send-dedupe` branch
(`480fc105`, `#448 send-dedupe`, in progress), which is cited by symbol because its line numbers will
move.

## The ask

The owner, 2026-10-23, paraphrased: take stock of what state is replicated between our client, our
server and the upstream chat service, and think more formally about merging it, in the way Pierce's
group did for file synchronization (Unison, Harmony). The client is not the only source of truth on
the vibe-talk side, so this is a synchronization problem between the vibe-talk view and the native
client's view of the same data. The motivating case: a reply that promotes its message into a thread
and is linked there while it is still sending. Is that a monotonic local action that eventually
synchronizes to the server? Whether a message was sent is something every replica must eventually
agree on. Read/unread state is private to the vibe-talk server and client, so it is synchronized
between those two parties only.

## Summary

1. **There is no peer-to-peer replication anywhere in vibe-talk.** The topology is two star-shaped
   links in series: provider ↔ server ↔ pages. Pages never talk to the provider. "Three-party" data
   is really two two-party links, and the server is the only replica that sees both.
2. **For message data the provider is the only real writer.** vibe-talk never edits, deletes or
   reacts upstream. Its one upstream write to messages is *create* (a post), plus moving the
   provider's read cursor forward. So message existence, content, edits, deletions, reactions and
   thread membership have one authority, and the server and page are caches of it. The merge
   problem there is not concurrent writers but **observations of different ages arriving out of
   order**. `cache/merge.rs` already solves that soundly on the server, using one clock (the
   server's own read-start times).
3. **The only operation with a genuinely uncertain outcome is a send.** A send is an operation the
   page invents, the server forwards, and the provider may or may not have applied when an answer is
   lost. That is where the triple send came from (`#448 send-dedupe`). The sound model is an
   **operation log keyed by the idempotency key**: a grow-only map from key to the provider message
   ids it produced, plus a monotone "may have posted" bit. Every replica converges on that map.
4. **Optimistic display should be derived, not written.** A pending reply, the thread it starts, its
   count and its colour should be computed at projection time as
   `provider state ⊔ pending operations ⊔ known replies`. They should never be written into the stored
   copy of the message. Written state needs a tombstone to take back and is overwritten by stale
   reads. Derived state is monotone for as long as the operation lives and disappears with no trace
   if it fails. This is the answer to the owner's question, worked through in §5.
5. **The vanished reply is a missing derivation.** On Slack a reply to a main message X is posted in
   X's thread. Neither the page's store nor the server cache then marks X as a root, so the reply sits
   in a thread no view draws. See §6, finding F2.
6. **Two-party state is mostly sound already.** Done has a read-your-writes overlay
   (`readSpeaksFor`). Bookmarks use compare-and-set revisions. Hues are first-writer-wins in one write
   transaction. Summaries are content-addressed. The upstream read cursor never moves backwards. The
   weaker points are small and are listed in §6.
7. **The changes, in order of value**, are in §7. The first four belong to the `#448 send-dedupe` and
   reply-promotes-thread lane. Items 3 and 4 there have already been sent to the coordinator as notes.

## 1. Replicas and topology

| Replica | Where it lives | Writes it originates |
|---|---|---|
| **U**, the provider | Slack, Discord or Google Chat, including every native client and other bot writing there. The native client is not observable separately, so it is part of U. | Every message create, edit and delete, every reaction, every thread, and the provider's own read cursor |
| **S**, the vibe-talk server | Durable store (`store/sqlite.rs`), disposable message cache (`cache/`), and in-memory state: live hub replay tail, Slack Flat memory, post lanes, post keys on the dedupe branch | Posts (creates) on the owner's or the voice agent's behalf, upstream read-cursor pushes, thread hue assignment, summaries |
| **P₁…Pₙ**, pages | Each open page: memory (`channelCanon`, `archivedIds`, the outbox) and `localStorage`. There is no IndexedDB. One device can run several pages, and the owner has several devices. | Intents: send, Done/put back, bookmark, pin, settings. Plus purely local state. |

Links:

- **U → S**: pushed live events (bridge adapter `POST /api/v1/live/events`, polling walks, Slack's
  tail on the `#452 slack-live-ingest` branch) and pulled reads (pages, windows, threads, the planned
  relist and backfill, `#453 cache-backfill`).
- **S → U**: posts (`ops::post_each`, `vibe-talk/src/ops.rs:3606`) and read-cursor moves
  (`upstream_clear.rs`, `reply_read.rs`).
- **S → P**: reads (timeline pages and deltas with `as_of`, `dismissed`, bookmark, hues and pins
  revision) and the SSE stream with a replay tail and `Last-Event-ID` resume
  (`vibe-talk/src/http/api.rs:2637`).
- **P → S**: explicit operations over HTTP. There is no state shipping from page to server.

## 2. Inventory

"Origin" means who may change it. "Propagation today" is the mechanism, with references.

### 2a. Three-party: provider, server, page

| State | Origin | Copies | Propagation today |
|---|---|---|---|
| **Message existence** | U (anyone). vibe-talk creates its own posts through U. | U; S cache `message` rows with `tombstone` rows (`cache/schema.rs:69,95`); P `channelCanon.messages` with `channelCanon.removed` (client tombstones, `voice.js:20059`), saved in `localStorage` `vibe-talk.voice.message-cache` | U→S by push create/delete, poll walks and listings. Absence from an exhaustive listing deletes (`merge.rs` rule 4, `absent_is_deleted`, `merge.rs:212`). Revival happens only by a read begun after the deletion (`revives`, `merge.rs:203`). S→P by stream create/delete and newest pages. The page drops a held message a newer page "answers for" and lacks (`pageFold`, `voice.js:20311`). |
| **Content and edits** | U | U; S `content`, `content_hash`, `edited_ms`, `observed_ms`, `content_partial`; P copy versioned by read-send order (`copyOrder`, `voice.js:24764`) | S: versioned register (`content_verdict`, `merge.rs:142`). P: a copy from a read sent later replaces one from a read sent earlier. Copies "as posted" (the stream's, the owner's own send) always yield to a read that carries the message (`postedCopies`, `voice.js:24797`). |
| **Thread membership** (a reply's thread id, a message's root-ness, reply count) | U. On Slack, a reply to X makes X a root. | U; S `message.thread_id`, `is_root` and the `thread` table (`cache/schema.rs:136`); P the copy's `thread` record and `channelCanon.threads` | S: non-content fields follow the newer observation (`cache/writer.rs:2087-2099`). A copy with no thread record keeps the held one. A posted reply touches only its own row (`observe::posted`, `cache/observe.rs:470`). P: membership is never invented from a live copy (`channelCanon.unplaced`, `#185`, `voice.js:20040`). Counts move with live replies only onto a root the store already holds as a root (`foldLiveMessage`, `voice.js:20559`). |
| **Reactions** | U only (vibe-talk cannot react) | U; S `extra.reactions`; the live hub's per-channel diff memory (`live.rs:171`); P copy | Whole set, newer observation wins, and a copy that could not see reactions never erases them (`writer.rs:2104`). Diff events on the stream (`#378 live-reactions`). |
| **Messages vibe-talk posts** | P (intent) or the voice agent (intent), carried out by S on U | P outbox `localStorage` `vibe-talk.outgoing-messages` (`voice.js:1915`); S in-flight post task and post lane; on the dedupe branch `PostKeys` in memory for 24 h; U the message | P→S `POST …/reply` with `idempotency_key` = outbox id. Today it is sent only to providers that honour keys. On the dedupe branch it goes to all providers. S→U `post_message_keyed` / `post_in_thread_keyed`. Part keys are bound to part text (`part_key`, `ops.rs:3760`). U→S→P: the 200 answer carries the parts, and the stream echo is marked `self_posted` (`note_self_posted`, `live.rs:460`). |
| **Provider read cursor** | U (the native client moves it, both ways). S moves it forward only. | U; S `upstream_clear` floor `pushed_*` and `reply_read` floor | S→U only. It is never read back into vibe-talk's read state (`upstream_clear.rs` header, `reply_read.rs` header). |

### 2b. Two-party: server and page

| State | Origin | Copies | Propagation today |
|---|---|---|---|
| **Done** (dismissals) | P (Done, put back, Undo, sent reply `#300`). S for the voice agent's sends (`reply_read::after_voice_send`). | S `dismissals` (`store/sqlite.rs:103`); P `archivedIds`, `channelCanon.dismissed`, saved in the message-cache key | P→S `POST …/dismiss` / `…/restore` through a serial queue (`queueInboxWrite`, `voice.js:38102`). Both are idempotent (`store/mod.rs:1934,1946`). S→P via `dismissed` on every timeline page. A read sent before a local write settles never overrides it (`localReadWrites`, `readSpeaksFor`, `voice.js:24705-24723`). A failed write takes back exactly what it flipped (`settleHeldDone`). |
| **Auto-read noise** | S rules (owner-edited) and exemptions | S `noise_rules`, `noise_exemptions`; computed per message on every read (`Message.noise`), never stored | Derived on S, served. |
| **"Own messages count as read"** | P setting per device | `localStorage` `vibe-talk.voice.mark-own-read` | Not synced (see Q5). |
| **Channel read mark** | API caller (the voice agent, MCP) via `POST …/read`. The page does not call it (no caller in `voice.js`). | S `read_marks` (`store/sqlite.rs:67`) | Monotone forward (`mark_read`, `store/mod.rs:1779`), but `DELETE …/read` forgets it. |
| **Reading bookmark** | P (Shift+B unconditional; Done/undo slides conditional) | S `bookmarks` with `revision` (`store/sqlite.rs:285`); P saved with the channel | Compare-and-set on `if_revision` for automatic moves; manual moves are unconditional (`decide_bookmark`, `store/mod.rs:1484`). The revision rides on every read. |
| **Thread hues** | S assigns (`#427`). P seeds once from its old local colours. | S `thread_hues` (`store/sqlite.rs:384`), bounded to 256 per channel, oldest evicted (`store/mod.rs:1548`); P `serverThreadHues` plus the fallback `vibe-talk.thread-hues` | First writer wins inside the write (`ensure_thread_hues`, `seed_thread_hues`, `store/mod.rs:2379,2402`). The page keeps the first server value it sees and lets it replace a provisional colour (`noteServerThreadHue`, `voice.js:22012`). |
| **Pins** (vibe-talk's own, not provider pins) | P | S `pins` with snapshots, `pin_revisions` | Set with a per-channel revision counter on reads. |
| **Summaries** | S (derived) | S `summaries` keyed by `(version, channel, message, content_hash)` | Content-addressed memo, dropped when the cache reports the text changed (`cache` `WriterEvent`). |
| **Synced settings**: voice settings, prompt, post prefix, noise rules, channel aliases, added and hidden channels, upstream-clear modes | P (Settings) | S single-row or per-channel tables; P memory | Whole-value writes, last arrival at S wins. "Never set" stays distinct from "set to the default" (no seeding, `store/sqlite.rs:165-172,298-303`). |
| **Voice transcript** | P (spoken turns) and S (sent records) | S `conversations`/`turns`; P `vibe-talk.voice.unsettled` for turns in progress | Append-only. An unsettled turn left by a dead page is filed once, by another page, marked cut off (`voice.js:17946-17975`). |

### 2c. Local to one page or device

Reply drafts (`vibe-talk.voice.drafts`), composer drafts (`vibe-talk.channel-drafts`,
`vibe-talk.voice.compose-draft`), dismissed sends kept for Undo (`vibe-talk.dismissed-sends`), the
outbox until its first POST, pack/unpack and card state (`vibe-talk.voice.threads-unpacked`,
`…thread-card-states`, `…thread-card-folded`, `…thread-order`, `…threads-main`), layout and reading
settings (columns, width, scale, collapsed lines, read speed, Enter sends, bar placement, link kinds,
single keys), UI state (`vibe-talk.voice.ui-state`, partly `sessionStorage`), derived identity and
author caches, the fallback hue memory, the token, the resume and relay state. The saved message
cache (`vibe-talk.voice.message-cache`) is local storage but is **a replica of S**, not local state.
It is the page's copy of §2a and §2b, trimmed (`trimCanonField`, `voice.js:19898`).

## 3. The Unison/Harmony lens

Unison's vocabulary: each replica has a current state. The synchronizer keeps an **archive**, the
state both sides last agreed on. The **update detector** compares each replica with the archive to
find what changed. **Reconciliation** propagates a change made on one side only. A path changed on
both sides differently is a **conflict**, which is shown to the user and skipped, never silently
resolved. Harmony adds schema-aware tree merging, where the hard part is **alignment**: deciding
which node on one side is "the same" as a node on the other.

| Unison concept | vibe-talk U→S (cache) | vibe-talk S→P (page store) | vibe-talk P→S (intents) |
|---|---|---|---|
| Archive | The cache rows plus `coverage` and `scope_head`: what S knew U held, over which stretch, as of which read start | `channelCanon` with each view's cover (`views`) | Not needed: operations are explicit |
| Update detector | Op-based: push events, poll walks. State-based: an exhaustive listing compared with coverage (rule 4). This is Unison's "unchanged in archive, missing in replica ⇒ deleted", bounded to what the listing could see (observed before it started, below the unsettled band). | Op-based: the SSE stream with replay. State-based: newest pages and deltas, compared by read-send order. | Each action is an operation. Done/restore are idempotent, bookmarks use compare-and-set, sends use a key. |
| Alignment | Provider message id | Message id; outbox id ↔ posted part ids (`entry.parts`, `outgoingObservations`) | The idempotency key |
| Conflict | None among writers, because U is the only writer of message data. "Conflicts" are races between observation paths, settled by versions (rules 2, 3 and 5). | As left | Done on device A vs put back on device B; bookmark auto vs manual; hue first writer |
| On a true conflict | U wins. vibe-talk cannot override the provider. | S wins, after the local overlay settles | S serializes. The loser's display is corrected at its next read. |

The useful consequence: **the only user data that can be lost in a conflict is the owner's own text
in the outbox**, because every other input either lives at U, which is authoritative, or is a small
idempotent intent. The rule for a true conflict is therefore Unison's: never drop it, show it. A send
that cannot land (its target removed, the channel made read-only) becomes a failed outbox row that
keeps its text, with Edit and Dismiss, drawn somewhere the reader can see it (see §5.4, race R2).

## 4. A formal model per class

Notation: a replica's state is a value in a join-semilattice (L, ⊔). Merging is ⊔. "Eventually agree"
means that once no new operations arrive and every message between linked replicas is delivered, all
replicas compute the same projection. Because the topology is a chain U–S–P, convergence of each link
implies convergence end to end, since ⊔ is associative.

### 4.1 Message rows: a versioned observation register per id (three-party)

For each message id `m`, S holds `(v, x)`, where `x` is either a row or a tombstone and `v` is a
version. Versions are ordered lexicographically:

1. **Provider edit time**, where both copies carry one, or where the provider always reports edits
   (`edit_order`, `merge.rs:131`). An earlier version never replaces a later one, however late it was
   seen.
2. Otherwise **observation start time on S's clock** (`observed_ms`, the time the read STARTED, or the
   provider's `as_of` if earlier). A push is observed at arrival.

Join on rows:

- Same text: keep, and **confirm** if a read brings text a push left partial (rule 5).
- Different text: the higher version wins. A pushed strict prefix of read text is a snippet and never
  wins (`is_snippet_of`).
- Tombstone `(deleted_ms)` against a row: a row wins only if it came from a read with
  `observed_ms > deleted_ms` (`revives`). A push never revives.
- **Absence is an operation too**: an exhaustive listing L of a scope over the stretch `[t0, t1)`,
  started at `ℓ`, joins `delete(m)` for every held `m` in its home scope with `observed_ms < ℓ` and
  `created < settled_below` (`absent_is_deleted`).

Why it is sound: every version component is either U's own (edit time) or S's one clock. A read
reflects U at some instant inside `[start, end]`. Taking `start` is conservative, because it never
claims to be newer than it is. With a single clock, "observed later" is a total order consistent with
causality at S. That makes this a last-writer-wins register with a causally safe timestamp, plus a
remove-wins tombstone with an epoch. Each piece is a semilattice, and so is their product.

The assumption it rests on: **a read started after U applied a change reflects that change.** It holds
for Slack's and Discord's own read APIs as far as we know. It is weakened by every server-side memory
that answers from older state: the Slack Flat memory (up to 15 min, `FLAT_RECONCILE`), bridge caches,
and the cache's own serving (`#384`). Each of those has to report its true age as `as_of`, and today
the Flat memory does (`merge.rs` rule 2 text). **Any new path that answers from memory must do the
same**, or it silently breaks rules 2 to 4. That includes `#452 slack-live-ingest`'s relist and
`#453 cache-backfill`.

Other fields (author, `reply_to`, thread record, reactions, mentions) follow the newer observation, and
a copy that lacks a field never erases it (`writer.rs:2087-2110`). For reactions that is an LWW
register of whole sets. It is safe because vibe-talk never writes reactions.

**The page uses the same shape with a different clock.** The page's version of a copy is the send
order of the read that brought it (`copyOrder`, `sentNumberOf`). That is the page's analogue of
`observed_ms`, and it is sound only if *a read the page sends later is answered from S-state at least
as new*. That fails exactly when S answers from memory: the Flat memory, a bridge cache, or the cache
serve path. The page then ranks a staler answer as newer. See finding F3.

### 4.2 Thread membership (three-party)

- **A reply's thread id is write-once.** A reply never moves between threads on any of the three
  providers. Model it as a write-once register: ⊥ (unplaced, `channelCanon.unplaced`) < `thread(T)`.
  The page's `unplaced` set is exactly this ⊥. It is sound.
- **Root-ness and the reply count are derived.** For providers where a reply is a thread reply
  (Slack, `channelRepliesInThread`, `voice.js:21810`):

  ```
  replies(T)      = { r : thread(r) = T } − tombstones          -- a 2P-set on reply ids
  root(X)         = U_says_root(X)  ∨  replies(threadOf(X)) ≠ ∅  ∨  pending(threadOf(X)) ≠ ∅
  threadOf(X)     = X.id                                          -- Slack: the root's ts is the thread id
  count(T)        = max(U_count(T) ?? 0, |replies(T)|) + |pending(T)|
  ```

  The deterministic `threadOf(X) = X.id` is what makes optimistic promotion cheap. The page can name
  the thread before the provider answers, so no temporary id ever needs reconciling. On Slack this
  is `continuation_thread` / `ids::ts_from_message_id` (`slack/client.rs:933`). Discord's
  thread-from-message also shares the message's id, though vibe-talk replies there by pointer, not
  thread. Google Chat messages already root a named thread.

- Root-ness is **not** stored monotonically upstream: a root whose replies are all deleted may stop
  being shown as one. Deriving it keeps that correct automatically, because when `replies(T)` empties
  the disjunct goes false.

### 4.3 Sends: a keyed operation log (three-party, the only uncertain writer)

An operation is `op = (k, channel, placement, text)`, with `placement ∈ {main, thread(T), reply_to(X)}`.
`k` is the outbox id, minted once per composed message and kept across attempts and reloads.

Per key, every replica holds a value in the product lattice

```
ids(k)      : G-Set of provider message ids this op produced        (grow-only)
attempted(k): Bool, false < true    -- a POST left; it may have posted   (= mayHavePosted)
refused(k)  : Bool, false < true    -- the provider answered no; certainly nothing posted
rest(k)     : the unsent suffix after a 207 (a new sub-operation, see below)
```

Projection: `posted ⇔ ids ≠ ∅` (all parts: `|ids| = parts`). `unconfirmed ⇔ attempted ∧ ids = ∅ ∧
¬refused`. `failed ⇔ refused ∧ ids = ∅`. Eventual agreement is about `ids(k)`. U holds the ground truth
(the messages exist or not). S learns it from the provider's answer or a later read. P learns it from
S's answer, the stream, or a read.

Retrying is safe iff a repeat of `k` either joins the same `ids(k)` or provably posts nothing. That
needs one of:

- **upstream idempotency** (the bridge nonce, `channelPostsOnce`): U itself deduplicates by part key;
- **S's memory of `k`** (`PostKeys` on the dedupe branch): exact while S remembers it, which today
  means in memory, for 24 h, until a restart;
- **an upstream update detector**: a read of the destination that finds the self-authored message
  with the same text posted after the attempt began (or carrying `k` in provider metadata). This is
  the only way to resolve a single part whose answer U lost on a provider without keys. Today's
  dedupe branch, correctly, does not pretend: it forgets such a post, and Retry may duplicate it.

A 207 splits the operation: the posted prefix's ids join `ids(k)`, and the unsent suffix is a new
operation with `text = rest`, `placement = thread(continueIn)`. The branch keys it by
`(k, …, rest)`, and `part_key` binds each part's upstream key to its text so the uncertain part is
asked for again as the same request (`unsent_remainder`, `ops.rs:3814`). That is sound.

**What identifies the operation must not include anything the page may legitimately change between
attempts.** Text is safe, because an Edit is a new message and should mint a new key (Q3). Placement
is not safe: reply-promotes-thread may learn or rewrite it. See finding F1.

### 4.4 Done: a server-serialized set with a session overlay (two-party)

S holds `D(channel) ⊆ ids`, written by `dismiss` (add) and `restore` (remove), in arrival order. P
holds `D_S` (as last read) plus a pending overlay `Δ = {id ↦ (pending writes, epoch)}`. The page
shows `D_S ⊕ Δ` and lets a read speak for `id` only once it was sent after `id`'s writes settled
(`readSpeaksFor`). That is the read-your-writes and monotonic-reads session guarantee of Bayou-style
systems. Across devices it is last-arrival-wins at S. That differs from an observed-remove set in one
case: Undo on device A of a Done that device B also made removes both. This is low value to fix
(§6, F7).

### 4.5 Bookmarks, hues, pins, settings, read marks (two-party)

- **Bookmark**: an LWW register whose clock is S's revision, which only grows, with compare-and-set for
  automatic moves. It is linearizable at S. Sound.
- **Hue**: a write-once register per `(channel, thread)`, so first writer wins: `⊥ < h`, and
  `h ⊔ h' = h` for the first. It is a lattice except that **eviction** at 256 per channel resets an old
  thread to ⊥, and it may get another colour later (F6). The page treats the first server value as
  final and lets it replace a provisional one. A page can be the first writer by seeding
  (`seed_thread_hues`), which matters in §5.
- **Pins**: an add/remove set with a revision counter. Same shape as Done, without the overlay.
- **Settings**: single-value LWW registers by arrival at S. Concurrent edits from two devices lose one
  (noise rules are written whole). Acceptable for a single owner.
- **Channel read mark**: a max-register (`mark_read` never moves back), but `forget` deletes the row.
  That is not a lattice: a stale mark sent after a forget re-advances it. It becomes one as
  `(epoch, max)` ordered lexicographically, where forget bumps the epoch. Only the voice agent and API
  use it (F8).
- **Provider read cursor**: S keeps a floor `F` and pushes only `m > F`, then sets `F = m`. That is a
  max-register replicated one way. The native client can move U's cursor back ("mark unread"). S never
  undoes that, and it re-advances only for a newer message. The owner's action in the native client
  wins until new traffic. That is the right answer for a cursor vibe-talk does not own.

### 4.6 Summaries and other derived state

`summary = f(version, content_hash)` is a memo of a pure function. Replicas converge trivially, and
invalidation only reclaims space. Auto-read noise, thread titles, mention names and hue fallbacks are
the same kind of state: derived, never needing synchronization of their own. **Derive wherever
possible** is the strongest single tool in this model, and §5 leans on it.

## 5. Optimistic client actions as monotone local operations

### 5.1 The principle

An optimistic action is an **operation entered into a local log**. Its effects come in two kinds, and
they must be kept apart:

- **Display effects** are a pure function of `(replicated state, pending operations)`: a phantom row,
  a promoted root, a count, a colour. They appear the moment the operation is logged. They disappear
  with no residue if it fails. They are never written into the replicated copy of anything.
- **Durable effects** on other replicas are applied only when the operation's outcome is known
  (`ids ≠ ∅`): Done on the message answered (`#300`, `markAnsweredDone`, `voice.js:41329`), the
  provider read cursor (`markReadInService`, `voice.js:41446`), a hue seed (the exception: see 5.3).

So the owner's question has a precise answer. **Yes: promoting a message into a thread is a monotone
local action, provided it is a display effect derived from the pending operation.** It is monotone in
the operation log, which only grows until an operation resolves. Resolution either keeps the effect
(the posted reply now justifies it through `replies(T)`) or removes the operation, and with it the
effect, cleanly. Nothing has to be "rolled back", because nothing was written.

### 5.2 The send lifecycle

```
composed ──queue──▶ queued ──(placing, if target unplaced)──▶ queued ──POST──▶ posting
posting ──200──────────────▶ posted(ids)                         [absorbing for ids]
posting ──207──────────────▶ posted-in-part(ids) + new op(rest)
posting ──4xx answered─────▶ failed (refused; nothing posted)    [Retry = same key, safe]
posting ──answer lost──────▶ unconfirmed (attempted)             [resolve before Retry]
unconfirmed ──outcome route / stream echo with key / upstream read──▶ posted(ids) | failed
reload during posting ─────▶ unconfirmed                         (outgoingFromSaved, voice.js:25852)
```

- **The idempotency key is the operation id** at every hop: outbox id on P, the claim in `PostKeys` on
  S, the part keys upstream where honoured.
- **Unify a pending row and the posted message by alignment, not by guessing.** The phantom row is
  `op:k`. When `ids(k)` becomes known from any source, the phantom is hidden and the real messages
  render in the same place (same scope, same thread, same colour). The invariant to test:
  **for every op, exactly one of {phantom, real messages} is drawn, never both and never neither**.
  Today the 200 answer is the only alignment source on the page. If it is lost, the stream echo of the
  same message arrives unaligned (it has `self_posted` but not the key). The page then shows a real
  message *and* an unconfirmed row offering Retry. The outcome route on the dedupe branch fixes this
  when it is asked. Carrying `k` on the self-posted stream event fixes it for free (§7, C2).
- **Rollback or keep-with-retry**: keep. A failed or unconfirmed operation stays in the log with its
  text. Its display effects stay too, marked as failed or unconfirmed, so the reply stays where it was
  written ("Not sent. Retry / Edit / Dismiss" in its thread). Its durable effects never happened. Only
  Dismiss removes the operation. That turns the display back to the replicated state, and the text
  goes to the dismissed-sends list for Undo (`#193`). The one exception is a destination that no longer
  exists (§5.4, R2).

### 5.3 Reply-promotes-thread, in detail (Slack)

Setting: Main shows message X with no thread. The owner presses Reply, writes, and sends. Op `k`:
`placement = reply_to(X)`, `T := threadOf(X) = X.id`.

At the moment of sending, all of this is derived and nothing is written:

| What is drawn | Derived from |
|---|---|
| X on Main as a root, "1 reply" chip marked sending | `root(X)` via `pending(T) ∋ k`; `count(T) = max(U_count, |replies|) + |pending|` |
| The reply as a phantom at the foot of thread T: in the right column, in the Threads view's card for T, under X where the inline reply was open | `op:k` placed in scope `thread(T)` |
| A card for T in the Threads view | `root(X)` |
| T's colour | See below |
| X not yet Done, the provider cursor not moved | Durable effects wait for `posted` |

On `200`: `ids(k) = {r}`. The phantom is hidden and r is drawn in the same place, because r's copy
says `thread = T` and the page folds it as the owner's own send. `pending(T)` loses k and `replies(T)`
gains r, so the count is unchanged, the chip stops saying "sending", and nothing moves. Then the
durable effects run: X Done, and the cursor marked through X.

**Colour.** Today a thread with no server hue gets the page's fallback colour, and the server's first
hue replaces it when it arrives (`noteServerThreadHue`). That is a guaranteed recolour of a thread the
owner just started. Better: at promotion the page picks `h` exactly as it picks a fallback, and seeds
it immediately (`POST …/thread-hues`, first writer wins at S). If S already had a colour (another
device started the same thread), S's answer wins and the page recolours once. That is a true conflict
resolved by the lattice. Otherwise `h` is final and never flickers. The seed is the one durable
effect worth taking before `posted`: a colour for a thread that never comes to exist is harmless, and
S evicts it in time.

**The rule for the stored copy of X.** The page must not write `is_root` or a thread record into
`channelCanon`'s copy of X, and must not add T to `channelCanon.threads`. Both are overwritten by any
read whose copy of X predates the reply. Today such a read is *ranked newer* if it was sent after the
promotion, because of read-send order. And taking them back on failure would need its own tombstone.
The projection functions (`projectView`, `mainRowMessages`, the thread-list builder, the count
helpers) take the outbox as a second input instead. The server cache follows the same rule in its own
way (§7, C8).

### 5.4 Races

| # | Interleaving | Outcome under the model |
|---|---|---|
| R1 | U edits X while k is pending | X's content merges as any edit does. The promotion depends on k, not on X's content. No interaction. |
| R2 | U deletes X **before** the POST reaches U | S resolves `reply_to(X)` by reading X (`reply_thread`, `slack/timeline.rs:1201`), finds it absent, and refuses (answered, nothing posted). k becomes failed with `refused`. Its display then has a destination that no longer exists: thread T of a removed root. **Draw the failed row where the reader is**: under the gap X left in Main, or the outbox foot, with "The message you answered was removed" (as `#315` already words it for placement). Never only inside thread T, which no longer exists. `root(X)` goes false with k's failure. X itself goes by its tombstone. |
| R3 | U deletes X **after** r landed | Slack keeps a placeholder root when a deleted message has replies (unverified for every client path; see Q6). The cache tombstones X's row (rule 3) but keeps r and the `thread` row. The page should draw T with a "message removed" root rather than dropping its replies. `root(X)` stays true via `replies(T) ∋ r`. |
| R4 | The owner deletes r in the native client after it posted | r is tombstoned. k is resolved (`ids = {r}`), so the phantom must **not** come back. "Phantom only while unresolved" guarantees it. `count(T)` drops by one, and `root(X)` falls if r was the only reply. |
| R5 | Someone else replies to X while k is pending | U makes X a root with their reply s. A read or the stream brings s with `thread = T`, the same T, because the id is deterministic. `count = max(U_count, |{s}|) + |{k}| = 2`, which is correct. After r lands, `max(2, |{s, r}|) + 0 = 2`. No flicker and no conflict. |
| R6 | The owner switches view (Main → thread T → Threads view → another channel → back) while k is pending | The outbox is per channel and persisted. Every projection re-derives the same display from `(channelCanon, outbox)`. Coming back reloads the store from `localStorage` and the outbox overlay re-applies. Nothing to reconcile. |
| R7 | The page reloads during `posting` | `outgoingFromSaved` makes k unconfirmed (`attempted`). The phantom shows as unconfirmed in T. The page should then ask the outcome route for every unconfirmed keyed op (§7, C3), and the stream replay with the key (C2) also aligns it. Today it waits for a human. |
| R8 | A read sent before the POST lands after the 200, carrying X without a thread record and no r | Display: unaffected, because derived state does not read X's thread record alone. Store: r was folded at `orderNow`, after that read was sent, so `copyOlderThan` keeps r. Correct today. |
| R9 | A read sent **after** the 200 is answered from older state (Slack Flat memory, a bridge cache, cache serve) and lacks r | `pageFold.answers` ranks r's copy older than that read (`copyOlderThan` without `says`), so r is dropped as deleted upstream until a later read brings it back. The reply vanishes for a while. This is F3. S's own rule 4 would *not* delete it, because r lies in the unsettled band of the newest end. The page has no such band. |
| R10 | The answer is lost, then the stream echo of r arrives | Today: r is drawn as a real message, and k shows unconfirmed with Retry. If Retry is pressed: on the dedupe branch S answers from `PostKeys` and posts nothing, unless S restarted meanwhile, which posts twice. With C2 the echo carries k, the page aligns, and Retry is never offered. |
| R11 | The page learns or changes k's placement between attempts (placing resolves `unknown`; promotion makes a later attempt say `thread_id = T`) | On the dedupe branch the fingerprint includes `thread_id` and `reply_to`, so a different placement under the same key is a **new post** (F1). Freeze the wire placement after the first attempt, or key by `k` alone. |
| R12 | Two devices each reply to X at once | Two operations with two keys and two replies. Both promote X. Hue: one seed wins and the other device recolours once. Both converge after reads. |

## 6. Where today's code stands

### Sound already

- **S's message merge** (`cache/merge.rs` with `writer.rs`): versioned register, epoch tombstones,
  absence bounded by coverage and the unsettled band, and snippet handling. It is the best-founded
  part of the system, and other replicas should copy its shape rather than invent new rules.
- **The page's read-your-writes overlay for Done** (`readSpeaksFor`, `markHeldDone`/`settleHeldDone`):
  a correct session guarantee, which takes back exactly what a failed write changed and never a stale
  snapshot.
- **Bookmark compare-and-set**, **first-writer hues inside one write**, **content-addressed
  summaries**, **the forward-only provider cursor**, **write-once thread placement with `unplaced` as
  ⊥** (`#185`), and **the conservative outbox reload** (a POST in flight at reload becomes unconfirmed
  and is never replayed).
- **The dedupe branch's core** (`post_keys.rs`): the claim is taken before the post lane, the post runs
  in its own task so it settles after its caller gives up, a concurrent repeat is coalesced, a
  certain-nothing post is forgotten, and a 207 is remembered with its rest. This is the right
  structure for "operation log keyed by `k`" on S.

### Ad hoc or unsound (findings)

- **F1. The dedupe fingerprint includes placement.** `post_keys::Fingerprint` is
  `(key, channel, thread, reply_to, text)`. An attempt whose `reply_to`/`thread_id` differs from the
  first (R11) claims a fresh slot and posts again. Today placement is stable after placing
  (`placeOutgoingMessage` sets `threadId` before any POST, `voice.js:26943`), so this is latent.
  reply-promotes-thread is exactly the change that can make it live. (Sent to the coordinator as a
  note.)
- **F2. The vanished reply: no replica promotes X.** The page folds the posted reply r into thread T,
  but `foldLiveMessage` (`voice.js:20559`) moves a count only on a root the store already holds with
  `is_root`, and X has no thread record, so Main draws X plain and r is not in Main. No view shows r
  until a read brings X's new copy. The server cache is the same: `observe::posted`
  (`cache/observe.rs:470`) offers only r's row. X's `is_root` changes only with a copy of X
  (`writer.rs:2099`). So a page served from the cache (`serve = true`) after the reply also gets X
  plain, beside an orphan reply. The fix is §5.3's derivation on P and C8 on S. (Sent to the
  coordinator as a note.)
- **F3. The page orders copies by its own read-send order, not by S's versions.** That is sound only
  while every S answer is at least as new as every earlier one. It is false for the Slack Flat memory,
  bridge caches and cache serve. R9 is the visible case, where an own reply is dropped for a while,
  and the same applies to content (an older text replacing a newer one until the next newest page).
  S knows each row's `observed_ms` and edit time. It should send them, and the page should compare
  those, as S does internally.
- **F4. The triple send (`#448`)** had no operation identity at S for providers without upstream keys.
  The dedupe branch fixes the main path. What remains: S's memory does not survive a restart (C5); a
  lost answer for a single part on Slack is still unresolvable without an upstream read (C6); a lost
  200 is not aligned by the stream echo (C2); unconfirmed entries after a reload wait for a human (C3).
- **F5. Promotion colour flicker.** Fallback colour, then the server's colour, for every new thread
  (§5.3). The seed route exists, but only migration uses it.
- **F6. Hue eviction breaks write-once.** Past 256 threads a channel's oldest colour is evicted, and a
  later read of that thread gets a new one. It is rare, and visible only for old threads.
- **F7. Done across devices is last-arrival-wins, not observed-remove.** Undo on A removes a dismissal
  B made of the same message. It is rare and low cost.
- **F8. `read_marks` forget is not monotone** (§4.5). Only the voice agent and API use it.
- **F9. Read-affecting settings are per device.** "Own messages count as read" lives in
  `localStorage`, so two devices can disagree about the same message's unread state, against the
  two-party contract that S and P agree on read state (Q5).

## 7. Changes, ordered by value

C1 to C4 belong to the `#448 send-dedupe` / reply-promotes-thread lane. C5 to C7 are server follow-ups.
C8 and C9 are small.

**C1. Identify a send by its key, not by its placement.** (F1, R11)
- Server: look a post up by `(key, channel, text)`. Record placement in the slot. A repeat whose
  placement differs from the recorded one is answered with the original outcome if the two are
  equivalent (`reply_to(X)` ≡ `thread(threadOf(X))` on a provider where replies are thread replies),
  and otherwise with `409 key_reused`. Never with a new post. The outcome route looks up the same way.
- Page: store the wire placement on the entry after the first attempt leaves (`entry.wire =
  {reply_to, thread_id}`) and send it unchanged on every retry of that key. Promotion changes only
  display. A 207's rest is already a new operation (new text, `continueIn`) and stays as it is.

**C2. Carry the key on the self-posted stream event, and align by it.** (F4, R10)
S knows `k ↔ r` when the provider answers (`post_each` has both; `note_self_posted(&one.id)`,
`live.rs:460`, is the place). Add `client_key: k` to the published event for messages this server
posted from a keyed request. A page holding op `k` joins `r` into `ids(k)` exactly as from a 200.
Other pages ignore unknown keys. This closes the lost-200 gap with no extra request, including across
a reload (the replay tail).

**C3. Resolve unconfirmed keyed entries on load.** (R7)
After `loadOutgoingMessages`, ask `POST …/reply/outcome` once for each `unconfirmed` entry with a key.
A 200 or 207 settles it. `post_unknown` leaves it unconfirmed as now.

**C4. reply-promotes-thread as a derived overlay.** (F2, F5, §5.3)
- Inputs: `channelCanon`, the outbox. Output: per scope, the rows to draw plus phantoms. `root(X)`,
  `count(T)` and `threadOf(X)` exactly as §4.2. Never write a thread record or a summary into the
  store for a pending operation.
- Only where `channelRepliesInThread(channel)`. Elsewhere `reply_to` is a pointer and promotes nothing.
- Hue: pick and seed at promotion. Take S's answer as final.
- Durable effects (Done on X, the provider cursor) after `posted`, as today.
- A failed operation whose destination root was removed is drawn in the parent scope (R2).
- Invariants for tests: §8.

**C5. Persist post keys in the store.** (F4)
A small table `post_keys(key, channel, text_hash, placement, outcome, at_ms)`, where `outcome` holds
part ids, the 207 facts and the unsent rest (the owner's own text, as `turns` already keeps). Bounded
by the same 24 h and count. A restart then answers a repeat of a landed post with its outcome instead
of `post_unknown`, and posts nothing. In flight at restart stays unknown, as it must.

**C6. Resolve an uncertain single part by reading upstream.** (F4)
When the only part's answer was lost on a provider without keys, read the destination (the thread,
or the newest page) before forgetting. Find a message by this account with the same text, posted at
or after the attempt's start. If found, remember it as posted. This is Unison's update detector
applied to our own operation. If the provider path can carry an opaque marker (Slack's
`chat.postMessage` documents a `metadata` field), put `k` there and match on it instead of on text
(Q4).

**C7. Send S's versions to the page.** (F3)
Add `observed_ms` (and the existing `edited_timestamp`) per message copy, and per newest page the
stretch it proves (the unsettled band's start, as `scope_head.unsettled_from_ms` already records).
The page then orders copies by S's version and applies rule 4's bound: a copy is dropped on absence
only when S observed it before the listing began **and** it is below the unsettled band. That retires
read-send order as the page's clock. Until then, a cheaper interim fix for R9: never drop an own
posted copy (`postedCopies` from the owner's send) on absence alone within a few minutes of its post.
Drop it only on an explicit deletion.

**C8. The server cache promotes the root of its own posted reply.** (F2, server half)
In `observe::posted`, for a reply whose `thread.id` names a root other than itself, also offer a
thread record for that root: `root_id = T`, reply count a lower bound (`reply_count_exact = false`),
activity at the reply's time. That keeps a served page consistent after an own reply. The root's
`is_root` stays as the next copy of the root says, under the same "derived from replies" reading as
§4.2.

**C9. Small lattice fixes.** Hue eviction: evict only threads not drawn recently, or keep evicted
colours as tombstones so they are reassigned the same hue (F6). `read_marks`: `(epoch, max)` (F8).
Done as an observed-remove set only if Q7 says it matters (F7).

**For `#452 slack-live-ingest` (alignment notes, not new work):** a content-carrying push from the
Slack tail should say whether its body is whole and carry `edited.ts`. Then rule 5 need not hold a
whole body as partial until a read, and `edit_order` can rank it. The periodic relist of each newest
page must be offered as a **listing** with coverage, not as rows, or it can never detect a deletion
(rule 4). Every answer from memory must report its true `as_of` (§4.1). Event ids must be stable across
the tail's retries (`LiveMessage.event_id`).

## 8. Testing: model-based, against random interleavings

The repository has no property-testing crate. The cache's race tests hold the writer at chosen points
(`cache/race_tests.rs`). The proposal fits that style: a small seeded generator (a 64-bit xorshift in
test code), so failures replay from a seed, plus a reference model.

### 8.1 The reference model

A pure model of U, S and P as maps, written for clarity, not speed:

- `U`: messages by id (text, edit time, thread, deleted). A per-account cursor. A key memory, for
  providers with keys.
- `S`: the real `merge.rs` functions are *already* the model of S's row merge. For the rest of S the
  model is "S's view = U's state at the latest read start that covered it, joined with pushes", which
  §4.1 defines.
- `P`: `projection(store, outbox)` → per scope, the rows and phantoms to draw.

The model's **convergence oracle**: after a quiescent tail (deliver every pending event, run one
newest read of each scope), every replica's projection equals `project(U, outbox)`.

### 8.2 Events to interleave

Upstream: create (another author), edit, delete, reaction change, reply to any message, delete a
root with replies. Server: deliver or delay a push, start a read (snapshot U now), finish a read
(possibly late, possibly from a stale memory with a declared `as_of`), restart (drop memory), lose a
provider answer after U applied the post. Page: send (main, reply to X, into T), Retry, Edit, Dismiss,
Done, put back, reload, switch view, lose the HTTP answer, deliver or replay a stream event.

### 8.3 Properties

1. **Exactly once.** For each op k, U holds at most one message per part produced under k, whatever
   the interleaving of Retry, lost answers, coalesced repeats and placement learning (C1). Restarts are
   included once C5 lands. Excluded until C6: a single part lost on a provider without keys, which the
   oracle labels "may duplicate" rather than "must not".
2. **Phantom xor real.** For each op k in every projection, exactly one of `op:k` or `ids(k)`'s
   messages is drawn, or neither only once k is dismissed or `ids(k)` are all deleted upstream (R4).
   This catches the vanished reply (F2), the double draw (R10) and R9.
3. **Promotion is monotone while pending.** From the moment k is logged until it resolves, `root(X)`
   is true in every projection, whatever reads land. After a failure it equals `U_says_root(X) ∨
   replies ≠ ∅`.
4. **Counts are steady.** `count(T)` never decreases while no reply of T is deleted upstream and no op
   of T fails. Specifically, it does not change when an op resolves to posted.
5. **No resurrection.** Once a delete of m is delivered, m is drawn again only after a read that
   started later shows it (rule 3, on P as on S).
6. **No reversion.** A message's drawn text never goes from a later version back to an earlier one.
   This catches F3 under stale reads.
7. **Read-your-writes.** After Done or put back on P, P's projection shows it until a read sent after
   the write settled says otherwise. Across two pages, both converge to S's last arrival.
8. **Durable effects gated.** Done on X and the provider cursor move happen only for ops with
   `ids ≠ ∅`, exactly once each.
9. **Colour stability.** A thread's drawn colour changes at most once in its life, and only when
   another device seeded it first (C4).

### 8.4 Where each runs

- **S properties (1, 5, 6 on the cache)**: Rust, in `tests/send_dedupe.rs`'s fake-provider harness
  (`tests/slack_fake`) and beside `cache/race_tests.rs`. The fake provider needs a "lose the answer
  after applying" switch and a restart of `AppState` on the same store.
- **P properties (2, 3, 4, 6, 7, 9)**: `tests/js/voice_page.test.mjs` already drives the page against a
  fake DOM with scripted server answers. Add a seeded driver that emits the §8.2 page and stream
  events, with server answers taken from the reference model, and checks the properties after every
  step and at quiescence. Shrinking can be by hand: print the seed and the event list, and trim it.
- **End to end (2, 3 on a real browser)**: one Chromium script for reply-promotes-thread, at
  1280x800 and 393x852, with the fake Slack answering the post slowly and the stream echo first. It
  asserts the reply is visible in some view in every frame from Send to settled.

## 9. Questions for the owner

1. **After an uncertain send on Slack, should the server look for the message upstream before
   offering Retry (C6)?** It costs one read per uncertain send, and it is the only way to make Retry
   always safe there.
2. **Which view should hold a failed reply whose message was removed upstream (R2)?** The model needs
   it drawn somewhere visible. The proposal is the gap in Main where the message was, with the outbox
   foot as the fallback.
3. **Should Edit of a message that may already have posted make a new message (a new key) and warn
   that the old one may be in the channel?** That is what the model needs. The alternative is to
   refuse Edit until the outcome is known.
4. **May vibe-talk put an invisible marker (its operation key) on the messages it posts, where the
   provider allows message metadata?** It would make C6 exact rather than a text match. Other
   integrations in the workspace could see the marker.
5. **Should read-affecting per-device settings sync through the server** ("own messages count as
   read", and arguably pack/unpack and card folds), so two devices agree on unread counts? Today they
   do not.
6. **When a thread's root is deleted upstream but its replies remain, should vibe-talk keep the
   thread with a "message removed" root (as Slack's own client does, as far as we know), or drop it?**
7. **Does Undo on one device need to leave a Done made on another device of the same message (an
   observed-remove set), or is last-action-wins fine?**
