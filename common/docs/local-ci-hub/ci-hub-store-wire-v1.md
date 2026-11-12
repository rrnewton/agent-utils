# ci-hub-store-wire/v1

Versioned wire contract between the upstream CI hub binary and a downstream
store connector running as a subprocess. The upstream binary owns semantics;
the downstream process owns transport and credentials. This contract is an M1
deliverable; its in-tree implementation is M2 and must not redesign it.

## 1. Transport and framing

- One long-lived subprocess per connector. Upstream spawns the configured
  command; the plugin reads requests on stdin and writes responses on stdout.
  Stderr is diagnostics only and is never parsed.
- JSONL: one JSON object per line, UTF-8, LF-terminated. No pretty-printing.
  Maximum line length is negotiated in `hello` (see §2); either side refuses
  an over-long line with `bad-request` and may then exit.
- Every request carries `"id"` (string, opaque, unique per connection) and
  `"op"`. Every single-line response echoes `"id"` and carries
  `"ok": true|false`. Multi-line `fetch` responses follow §3.4 instead.
- Requests are strictly sequential: at most one unacked request per
  connection. Upstream sends the next request only after the previous
  response completes (for `fetch`, after its `end` or `abort` line). A
  response with an unknown id is a protocol error; upstream closes the
  connection and reports `unavailable`. A plugin may close the connection on
  timeout; upstream likewise reports `unavailable` and may respawn it.
- `"protocol": "ci-hub-store-wire/v1"` appears in `hello` only; later
  messages rely on the negotiated connection.

## 2. Hello and versioning

First exchange on every connection:

Request:

```json
{"id":"1","op":"hello","protocol":"ci-hub-store-wire/v1","role":"record","client":"local-ci-hub/<version>"}
```

Response:

```json
{"id":"1","ok":true,"protocol":"ci-hub-store-wire/v1","role":"record","connector":"<name>/<version>","limits":{"max_events_per_append":1000,"max_append_bytes":67108864,"max_events_per_read":1000,"max_read_bytes":16777216,"max_line_bytes":67108864,"max_event_line_bytes":67108864,"max_object_bytes":50331648}}
```

- `role` is `record|publish|local`, as configured upstream. The plugin must
  echo the role it accepted; a mismatch is `bad-request`.
- Unknown major version (anything but `ci-hub-store-wire/v1`) refuses with
  class `version-unsupported`. Minor evolution is additive fields only;
  unknown fields are ignored by both sides.
- `limits` are the plugin's hard bounds. Upstream never exceeds them:
  - `max_events_per_append`, `max_append_bytes`: append batch bounds (§3.1).
  - `max_events_per_read`, `max_read_bytes`: read page bounds (§3.3).
    `max_read_bytes` defaults to 16 MiB when a deployment does not set it.
  - `max_line_bytes`: any single wire line, either direction.
  - `max_event_line_bytes`: one event `line` including JSON escaping (§3.1).
  - `max_object_bytes`: one evidence object (§3.4), default 48 MiB
    (50331648). A plugin that cannot accept the deployment's required cap
    must say so in `hello`; upstream treats a lower cap as a configuration
    error, never as a silent truncation.

## 3. Operations

Semantics are those of the in-process `Store` trait. Wire field names below
are frozen by this contract.

### 3.1 append(batch)

Request:

```json
{"id":"2","op":"append","events":[{"event_id":"<id>","line":"<event JSON as one string>"}]}
```

- `line` is the exact event line bytes, canonical by upstream construction.
  The plugin stores bytes; it does not re-canonicalize. Duplicate identity
  on the wire is exact `line` byte equality (§6); any pair-identity
  equivalence is upstream's job before send. A plugin may store
  `sha256(line)` per `event_id` to decide duplicates.
- Batch bounds: at most `max_events_per_append` events and
  `max_append_bytes` total `line` bytes; each `line` at most
  `max_event_line_bytes` including JSON escaping. Upstream splits larger
  batches and refuses to emit an event line above the bound. Historical fat
  lines (for example backfill rows) never travel as append lines: they move
  through the evidence path and are fetched by digest (§3.4).

Response (one ack per event, in request order):

```json
{"id":"2","ok":true,"acks":[{"event_id":"<id>","status":"stored"}]}
```

- `status` is `stored|duplicate|refused`. A refused ack carries `"class"`
  (§5) and `"message"`.
- Append is per-event, not all-or-nothing: a partial batch MAY store.
  `ok:true` with a refused ack means the batch was processed; that event is
  not stored on this connector. Upstream keeps the spool for every refused
  or unacked event and replays only those. The record connector's ack set is
  the visibility boundary: an event is durably visible exactly when the
  record connector has acked it `stored` or `duplicate`.
- Acks are per-connector and independent. A publish ack never stands in for
  the record ack.

### 3.2 head()

Request: `{"id":"3","op":"head"}`

Response:

```json
{"id":"3","ok":true,"cursor":{"authority":"<connector-id>","head":"<head-token>","shards":{"<shard-path>":{"offset":0,"prefix_sha256":"<hex>"}}}}
```

- `head` is opaque to upstream except for equality and the validity rules in
  §4 (git: commit id; record store: its head token).

### 3.3 read_from(cursor, limit)

Request:

```json
{"id":"4","op":"read_from","cursor":{},"limit":1000}
```

Response:

```json
{"id":"4","ok":true,"events":[{"shard":"<path>","offset":123,"line":"..."}],"next_cursor":{},"more":true}
```

- Returns each shard's bytes after `offset`, in shard path order, split into
  stored lines. A page ends at whichever bound hits first: `limit` events,
  the plugin's `max_events_per_read`, or `max_read_bytes` of returned `line`
  bytes. Counting bytes as well as events keeps a page of fat lines from
  becoming one unbounded response. `more:true` means call again with
  `next_cursor`.
- `offset` per event is the byte offset of that line's first byte in its
  shard. `next_cursor` advances past exactly the returned bytes.
- Cursor validity and reset follow §4. An invalid cursor is not an empty
  read: the response carries `cursor_reset` (§4) or refuses (§5).

### 3.4 fetch(sha256, max_bytes) — bounded, streaming, verified

Request: `{"id":"5","op":"fetch","sha256":"<hex>","max_bytes":50331648}`

Response is multi-line, same id. Only the `begin` line carries `ok`;
`chunk`, `end`, and `abort` lines carry no `ok` field:

```json
{"id":"5","ok":true,"phase":"begin","declared_len":1590000,"sha256":"<hex>"}
{"id":"5","phase":"chunk","data_b64":"..."}
{"id":"5","phase":"end","bytes":1590000}
```

- `declared_len` is known before the first byte. An object above `max_bytes`
  is refused unread with class `object-too-large` (single-line response,
  `ok:false`, no `begin`).
- Chunks are base64, each at most 1 MiB decoded. The receiver checks SHA-256
  at end of stream; a mismatch, an overrun past `declared_len`, or a short
  stream refuses with `object-digest-mismatch`. Nothing returns a whole
  object as one field.
- Any failure after `begin` is a single abort line,
  `{"id":"5","phase":"abort","class":"<class>","message":"..."}` (§5), and
  upstream discards all partial bytes. There is no resume within a fetch; a
  retry starts a new fetch.
- Missing object: `object-missing` (single-line, `ok:false`). None of these
  is ever a test result.

## 4. Cursor rules

Cursor = `{authority, head, shards: {path -> {offset, prefix_sha256}}}`,
where `prefix_sha256` is the SHA-256 of the first `offset` bytes of that
shard.

- Normal tail: new shards start at offset 0 (new month, new `.part-NNNN`,
  new host, new team). Multi-part and multi-host sets need no special case.
- Validity: current head descends from `cursor.head` AND every shard's first
  `offset` bytes still hash to `prefix_sha256`.
- Squash or head rewrite (non-descendant head): cursor invalid. The reader
  re-verifies each indexed prefix by digest against the new head's manifest
  and rescans each shard from the first unverifiable byte (byte 0 when a
  prefix differs or a shard is gone). After a pure squash every prefix still
  verifies, the rescan is empty, and the cursor rebinds to the new head.
  Never a silent skip: the answer carries
  `"cursor_reset":{"reason":"head-rewrite|prefix-mismatch|shard-removed"}`.
- Rewrite under a fast-forward head (shard shrinks or prefix changes):
  append-only violation. Rescan that shard from 0 and report refusal class
  `store-rewrite` until an operator acknowledges it. Acknowledgement is
  out-of-band in v1 (no `ack_rewrite` op): the operator deletes or rewinds
  the deployment's stored cursor state, and the next `head()` starts fresh
  from the rewritten authority.
- Event order is not shard order. The fold sorts by `(emitted_at, event_id)`,
  independent of shard/file/line order. Shard order fixes only scan order
  and which conflict is reported first. Late rows (older `emitted_at` in new
  suffixes) re-fold every touched run in total order.

On the wire, `read_from` with an invalid-but-recoverable cursor returns
`ok:true` with `cursor_reset` and the rescanned events; with an
unrecoverable cursor it returns `ok:false`, class `cursor-invalid` (or
`store-rewrite` for the fast-forward violation).

## 5. Refusal classes

Disjoint from validation results and from each other. None is ever a test
red, and none is ever green. Single-line error shape:
`{"id":"<id>","ok":false,"class":"<class>","message":"..."}`. Per-event
refusals use the ack shape in §3.1 with the same classes; mid-fetch failures
use the `abort` line in §3.4.

| class | meaning |
|---|---|
| `unavailable` | connector unreachable, timed out, or connection lost; retry may succeed |
| `auth` | credentials missing or rejected |
| `rate-limited` | throttled; back off and retry |
| `bad-request` | malformed request or negotiated-bound violation |
| `version-unsupported` | unknown protocol major version |
| `conflicting-event-id` | same event_id, different line bytes (see §6) |
| `cursor-invalid` | cursor cannot be honoured or rebound |
| `store-rewrite` | append-only violation under a fast-forward head |
| `object-missing` | no object for the requested digest |
| `object-too-large` | declared length above max_bytes; refused unread |
| `object-digest-mismatch` | streamed bytes fail length or SHA-256 check |

## 6. Idempotent replay

- Identity on the wire is exact `line` bytes, canonical by upstream
  construction. Re-sending an identical `line` for the same `event_id` acks
  `duplicate`. It is never stored twice.
- Re-sending the same `event_id` with different `line` bytes acks `refused`
  with class `conflicting-event-id`, naming the id. It is never an
  overwrite. Upstream guarantees that two lines it considers the same event
  (written-pair identity in the core) are sent as identical bytes, so byte
  equality never splits one event into a false conflict.
- Replay after a crash is therefore safe: upstream re-sends unacked events
  only; stored events ack `duplicate`, new events ack `stored`, and refused
  events stay spooled (§3.1).

## 7. Conformance transcripts

Synthetic request/response pairs live under
`rs/local-ci-hub/tests/fixtures/wire/` and are run by the crate's test suite:
hello ok, hello version refusal, append stored/duplicate/conflicting,
partial-batch store (one stored, one refused, spool replay of the refused
event only), over-long event line refusal, head, read_from paging by event
limit, read_from paging by byte budget, read_from cursor_reset (squash),
read_from store-rewrite, fetch ok (chunked), fetch abort mid-stream with
partial bytes discarded, fetch too-large, fetch missing, fetch digest
mismatch, and one refusal-isolation case (a store refusal never surfaces as
a verdict). M2 runs the same fixtures against the in-tree implementation.
