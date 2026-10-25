//! Durable on-disk prompt queues and process identity for interactive coding agents.
//!
//! A program that types prompts into an agent's terminal composer must never type one prompt
//! twice, must never lose one, and must be able to say afterwards exactly what happened to each.
//! This library holds the parts of that contract that are about files and processes, not about
//! terminals, so that several programs (a drainer that types, an enqueuer that only writes, a
//! status reader) agree on one format:
//!
//! - [`queue`]: the queue directory, its phases, message identifiers, enqueue with its
//!   duplicate check, state lookup, withdrawal, queue order, and quarantine.
//! - [`fsio`]: owner-only directories and locks, crash-safe JSON writes and no-replace renames.
//! - [`identity`]: a process generation's identity, whether it is still alive, and whether it is
//!   an ancestor of the caller.
//!
//! Typing into a terminal, readiness checks and the target lock that serializes typists across
//! queues belong to the drainer, not to this library.
//!
//! # On-disk format, version 1
//!
//! A queue is one directory, mode 0700 and owned by the user. Every file in it is JSON written
//! by [`fsio::atomic_json`] or [`fsio::atomic_json_create`] (mode 0600, synced, renamed or
//! linked into place, directory synced):
//!
//! ```text
//! QUEUE/
//!   target.json          the binding: which terminal this queue types into
//!   .delivery.lock       flock: drains, explicit-identifier enqueues and withdrawals
//!   .binding.lock        flock: changes to target.json
//!   inbox/<id>.json      pending: never typed
//!   inflight/<id>.json   past the at-most-once barrier, outcome unsettled
//!   processed/<id>.json  delivery confirmed
//!   failed/<id>.json     quarantined, with failed/<id>.json.error beside it
//!   */.message.<pid>.<n> temporaries of an atomic write in progress
//! ```
//!
//! **Identifiers.** `<id>` is 1-255 ASCII letters, digits, `.`, `_` or `-`, starting with a
//! letter or digit ([`queue::validate_message_id`]). A generated identifier is the wall-clock
//! time in nanoseconds (20 digits), `-`, and the process identifier. An item's `id` field, when
//! present, equals its file name without `.json`.
//!
//! **One identifier, one phase.** An identifier exists in at most one phase directory. Enqueue
//! refuses an identifier present in any phase ([`queue::enqueue`]); with an explicit identifier
//! it holds the delivery lock across that check. Items move between phases only by rename,
//! never replacing a file ([`fsio::transition`]). Withdrawal deletes an inbox item, so its
//! identifier may be queued again: the queue is not a permanent record of every identifier.
//!
//! **Lifecycle.** A drainer holds the delivery lock for the whole drain. For each inbox item, in
//! queue order, it waits for the target to be ready, renames the item to `inflight/` and records
//! `possibly_submitted: true` **before** typing, then types it. A confirmed delivery moves it to
//! `processed/`; an outcome that may have reached the terminal moves it to `failed/`, never back
//! to the inbox; a refusal proven to precede any input moves it back to `inbox/`. A drain that
//! finds items left in `inflight/` by a crashed drainer quarantines them as
//! `possibly_submitted` ([`queue::recover_inflight`]): no item is ever replayed automatically.
//!
//! **Queue order** ([`queue::inbox_in_queue_order`]): ascending numeric `queued_at`, then file
//! name; items without a numeric `queued_at` (or unreadable ones, which the drain quarantines)
//! come first, by name.
//!
//! **Item fields.** Required: `text` (nonempty string). Written at enqueue: `id`, `queued_at`
//! (Unix seconds, fractional) and `delivery_attempts` (0). A drainer may add `delivery_state`
//! (`pending`, `inflight`, `processed`), `delivery_error`, `delivery_blocked_at`,
//! `possibly_submitted`, `inflight_at`, `confirmed_at`, `delivery_failed_at`,
//! `delivery_attempts` and its alias `tui_delivery_attempts`, `misroutes` (array of
//! `{at, error}`) and `probable_misroute`. A sender may add fields of its own, such as the
//! original send time of a chat request (`sent_at`, `opening`). Readers ignore fields they do not
//! know, so a field cannot make an older reader refuse an item.
//!
//! **Quarantine sidecar** `failed/<id>.json.error`: `{artifact, outcome, error, failed_at}`, where
//! `outcome` is `possibly_submitted` or `invalid_message`.
//!
//! **Target binding** `target.json`, written once by the first send and changed only under both
//! locks when the terminal provably moved: `{"kind": "pane", "pane_id", "expected_agent",
//! "expected_workspace", "expected_cwd"}` or `{"kind": "session", "agent", "value",
//! "expected_agent", "expected_workspace", "expected_cwd"}`, absent expectations being `null`.
//! A drainer refuses a queue bound to a different target.
//!
//! **Timestamps** in items and sidecars are Unix seconds as JSON numbers.

pub mod error;
pub mod fsio;
pub mod identity;
pub mod queue;

pub use error::{Error, ErrorKind, Result};
