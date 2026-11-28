//! Messages too large for one relay post, sent in parts (#2420 R12 relay-never-lossy).
//!
//! The sender's first post is the usual envelope with its body cut to part 1, plus
//! `body_parts: N` and `part_sha256: [h1..hN]`; `body_sha256` stays the whole body's hash, and
//! `relay.sig3` signs the parts list (see `envelope_material_parts`). Parts 2..N follow as their
//! own posts: `{schema: agent-teams-body-part/v1, id, index, of, text}`. Sending in parts is off
//! unless the relay route sets `chunk_posts: true`.
//!
//! The receiver keeps each message's parts in `relay/body-parts/<id>.json` until all have
//! arrived, checks each part against the signed list, and delivers the joined body once through
//! the ledger. Nothing is dropped:
//! - A part still missing 10 minutes after the first post: the verified parts are delivered,
//!   with `parts 1-2 of 4 verified; 3-4 missing; reconfirm before acting` on the first line.
//!   Parts that arrive after that are a SECOND delivery, carrying the late parts and the
//!   whole-body hash verdict.
//! - A part that does not match its signed hash, a second different text for one part, a part
//!   whose index or count does not fit the signed list, or an orphan part (no envelope within
//!   24 hours) is delivered on its own as UNVERIFIED: no proof, a first-line warning, and the
//!   tag names only the relay bot that posted it (anyone may post into a channel).
//! - Assembly state is bounded per poster and in total; a poster that hits a bound has ITS
//!   oldest assembly delivered at once as a partial, never another poster's.

use super::{
    deliver_raw_unverified, deliverer_for, is_confirmed_sender, load_declarations, lock_store,
    log_event, record_unparseable_stranger, relay_in_envelope, render_post_with_limit,
    restore_slack_bullets, result, sign_envelope_parts, strip_provider_notes,
    undo_slack_link_markup, verify_envelope_parts, RelayInResult, NOT_RECORDED,
};
use crate::envelope::{render_with_warnings, sha256_hex, valid_message_id};
use crate::jsonfile::{write_json_atomic, DataResult};
use crate::timeutil::{format_rfc3339_ms, now_unix_ms};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The schema of a body-part post.
pub const BODY_PART_SCHEMA: &str = "agent-teams-body-part/v1";
/// The fence label of a body-part post.
pub const BODY_PART_FENCE: &str = "agent-teams-body-part";
/// Most parts one message may be sent in.
pub const MAX_PARTS: usize = 128;
/// The largest body a message sent in parts may carry.
pub const MAX_CHUNKED_BODY_BYTES: usize = 256 * 1024;
/// How long after the first post a missing part makes the receiver deliver what it has.
pub const PARTIAL_AFTER_MS: i64 = 10 * 60_000;
/// How long an assembly (or an orphan part) is kept.
pub const ASSEMBLY_KEEP_MS: i64 = 24 * 3_600_000;
/// Most assemblies in flight for one poster.
pub const MAX_ASSEMBLIES_PER_SENDER: usize = 16;
/// Most bytes held in all assemblies together.
pub const MAX_ASSEMBLY_BYTES: usize = 8 * 1024 * 1024;
/// Largest text one part post may carry.
const MAX_PART_TEXT_BYTES: usize = 64 * 1024;
/// Smallest part a sender cuts a body into.
const MIN_PART_BYTES: usize = 200;
const ASSEMBLY_SCHEMA: &str = "agent-teams-body-parts/v1";

// ---------------------------------------------------------------------------------------------
// Sender

/// Cut `body` into parts of at most `max_part_bytes`, at a line end when one falls in the second
/// half of the part, else at a character boundary.
pub fn split_body(body: &str, max_part_bytes: usize) -> Vec<String> {
    let max_part_bytes = max_part_bytes.max(4);
    let mut parts = Vec::new();
    let mut rest = body;
    while rest.len() > max_part_bytes {
        let mut cut = max_part_bytes;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if let Some(newline) = rest[..cut].rfind('\n') {
            if newline + 1 >= cut / 2 {
                cut = newline + 1;
            }
        }
        parts.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    parts.push(rest.to_string());
    parts
}

/// One body-part post: a readable first line, then the part in its fence.
pub fn render_part_post(id: &str, index: usize, of: usize, text: &str) -> String {
    let part =
        json!({"schema": BODY_PART_SCHEMA, "id": id, "index": index, "of": of, "text": text});
    format!(
        "[agent-teams body part {index}/{of} of {id}]\n```{BODY_PART_FENCE}\n{}\n```\n",
        super::fence_json(&part)
    )
}

/// The posts that carry `envelope` in parts, each within `max_post_bytes` after the
/// publisher's escaping: the chunked envelope first (signed with `relay.sig3`), then parts 2..N.
pub fn chunked_posts(
    state: &Path,
    envelope: &Value,
    max_post_bytes: usize,
) -> Result<Vec<String>, String> {
    let body = envelope["body"].as_str().unwrap_or_default();
    if body.len() > MAX_CHUNKED_BODY_BYTES {
        return Err(format!(
            "body of {} bytes is over the {} bytes a message sent in parts may carry",
            body.len(),
            MAX_CHUNKED_BODY_BYTES
        ));
    }
    let id = envelope["id"].as_str().unwrap_or_default();
    let mut part_bytes = max_post_bytes / 2;
    while part_bytes >= MIN_PART_BYTES {
        let parts = split_body(body, part_bytes);
        if parts.len() > MAX_PARTS {
            break;
        }
        if parts.len() >= 2 {
            let mut chunked = envelope.clone();
            chunked["body"] = json!(parts[0]);
            chunked["body_parts"] = json!(parts.len());
            chunked["part_sha256"] = json!(parts
                .iter()
                .map(|part| sha256_hex(part.as_bytes()))
                .collect::<Vec<_>>());
            sign_envelope_parts(state, &mut chunked).map_err(|error| error.to_string())?;
            let mut posts = vec![render_post_with_limit(&chunked, max_post_bytes)];
            for (index, text) in parts.iter().enumerate().skip(1) {
                posts.push(render_part_post(id, index + 1, parts.len(), text));
            }
            if posts
                .iter()
                .all(|post| super::measured_post_bytes(post) <= max_post_bytes)
            {
                return Ok(posts);
            }
        }
        part_bytes /= 2;
    }
    Err(format!(
        "envelope does not fit {MAX_PARTS} posts of {max_post_bytes} bytes"
    ))
}

// ---------------------------------------------------------------------------------------------
// Receiver: assembly state

fn assembly_dir(state: &Path) -> PathBuf {
    state.join("relay").join("body-parts")
}

/// `relay/body-parts/<id>.json`; the ID is checked by the caller before any path is built.
fn assembly_path(state: &Path, id: &str) -> PathBuf {
    assembly_dir(state).join(format!("{id}.json"))
}

fn load_assembly(state: &Path, id: &str) -> Option<Value> {
    std::fs::read_to_string(assembly_path(state, id))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(|doc| doc.get("schema").and_then(Value::as_str) == Some(ASSEMBLY_SCHEMA))
}

fn save_assembly(state: &Path, id: &str, doc: &Value) -> DataResult<()> {
    crate::jsonfile::create_private_dir_all(&assembly_dir(state))?;
    write_json_atomic(&assembly_path(state, id), doc)
}

fn remove_assembly(state: &Path, id: &str) {
    let _ = std::fs::remove_file(assembly_path(state, id));
}

fn all_assemblies(state: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(assembly_dir(state)) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(|doc| doc.get("schema").and_then(Value::as_str) == Some(ASSEMBLY_SCHEMA))
        .collect();
    out.sort_by_key(|doc| doc["first_at"].as_i64().unwrap_or(0));
    out
}

fn new_assembly(id: &str, route: &str, peer_sender: &str, channel: &str, now: i64) -> Value {
    json!({
        "schema": ASSEMBLY_SCHEMA, "id": id, "route": route, "peer_sender": peer_sender,
        "channel": channel, "thread": "", "to_team": null, "post_text": "",
        "first_at": now, "last_at": now, "envelope": null, "parts": {}, "orphans": [],
        "state": "assembling", "delivered_parts": [],
    })
}

/// Bytes an assembly holds (part and orphan texts).
fn held_bytes(doc: &Value) -> usize {
    let parts: usize = doc["parts"]
        .as_object()
        .map(|parts| parts.values().filter_map(Value::as_str).map(str::len).sum())
        .unwrap_or(0);
    let orphans: usize = doc["orphans"]
        .as_array()
        .map(|orphans| {
            orphans
                .iter()
                .filter_map(|orphan| orphan["text"].as_str())
                .map(str::len)
                .sum()
        })
        .unwrap_or(0);
    parts + orphans
}

fn part_count(doc: &Value) -> usize {
    doc["envelope"]["body_parts"].as_u64().unwrap_or(0) as usize
}

fn part_hash(doc: &Value, index: usize) -> Option<String> {
    doc["envelope"]["part_sha256"]
        .as_array()?
        .get(index.checked_sub(1)?)?
        .as_str()
        .map(str::to_string)
}

fn verified_parts(doc: &Value) -> Vec<usize> {
    let mut found: Vec<usize> = doc["parts"]
        .as_object()
        .map(|parts| parts.keys().filter_map(|key| key.parse().ok()).collect())
        .unwrap_or_default();
    found.sort_unstable();
    found
}

/// `text`, or its Slack link-markup and bullet restorations, when one hashes to `expected`.
fn restore_part(text: &str, expected: &str) -> Option<String> {
    if sha256_hex(text.as_bytes()) == expected {
        return Some(text.to_string());
    }
    let unlinked = undo_slack_link_markup(text);
    if unlinked != text && sha256_hex(unlinked.as_bytes()) == expected {
        return Some(unlinked);
    }
    restore_slack_bullets(text, expected).or_else(|| {
        (unlinked != text)
            .then(|| restore_slack_bullets(&unlinked, expected))
            .flatten()
    })
}

/// Index ranges as `1-2, 4`.
fn ranges(indices: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut iter = indices.iter().copied().peekable();
    while let Some(start) = iter.next() {
        let mut end = start;
        while iter.peek() == Some(&(end + 1)) {
            end += 1;
            iter.next();
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
    }
    out.join(", ")
}

/// Make room under the bounds for one more assembly or part from `peer_sender`: while that
/// poster would hold more than MAX_ASSEMBLIES_PER_SENDER assemblies, or the total would pass
/// MAX_ASSEMBLY_BYTES, ITS oldest assembly (never another poster's) is marked to be delivered at
/// once as a partial, naming the bound (lead-014). `keep` (the assembly being added to) is never
/// marked here; when nothing else of the poster's is left to set aside and the total is still
/// over, `keep` itself is named (the second value) to be delivered early, since another poster's
/// assembly must never make way for it. Returns the IDs marked; the caller delivers them once the
/// store lock is released. The caller holds the store lock.
fn make_room(
    state: &Path,
    peer_sender: &str,
    keep: &str,
    adding: usize,
) -> (Vec<String>, Option<&'static str>) {
    let all = all_assemblies(state);
    let mine: Vec<&Value> = all
        .iter()
        .filter(|doc| doc["peer_sender"].as_str() == Some(peer_sender))
        .filter(|doc| doc["state"].as_str() != Some("complete"))
        .filter(|doc| doc["id"].as_str() != Some(keep))
        .collect();
    let mut count = mine.len() + 1;
    let mut total: usize = all.iter().map(held_bytes).sum::<usize>() + adding;
    let mut marked = Vec::new();
    for doc in mine {
        let bound = if count > MAX_ASSEMBLIES_PER_SENDER {
            "16 assemblies per poster"
        } else if total > MAX_ASSEMBLY_BYTES {
            "8 MiB of assemblies"
        } else {
            break;
        };
        let Some(id) = doc["id"].as_str().map(str::to_string) else {
            continue;
        };
        let mut marked_doc = doc.clone();
        marked_doc["force_partial"] = json!(bound);
        if save_assembly(state, &id, &marked_doc).is_ok() {
            count -= 1;
            total = total.saturating_sub(held_bytes(doc));
            marked.push(id);
        }
    }
    // Still over the total with nothing else of this poster's to set aside: the assembly being
    // added to is this poster's oldest, so it is the one delivered early (lead-014: never
    // another poster's).
    let keep_bound = (total > MAX_ASSEMBLY_BYTES).then_some("8 MiB of assemblies");
    (marked, keep_bound)
}

/// Deliver an assembly a bound marked: a forced partial when its envelope is held, else its
/// orphan parts as UNVERIFIED; the record goes either way.
fn deliver_marked(state: &Path, this_host: &str, id: &str, now: i64) {
    let Some(doc) = load_assembly(state, id) else {
        return;
    };
    if doc["envelope"].is_null() {
        deliver_orphans(
            state,
            this_host,
            &doc,
            "set aside early, its poster reached a bound",
        );
        remove_assembly(state, id);
    } else {
        let _ = progress(state, this_host, id, now);
    }
}

/// Deliver an orphan-only assembly's parts, each on its own as UNVERIFIED.
fn deliver_orphans(state: &Path, this_host: &str, doc: &Value, why_prefix: &str) -> usize {
    let id = doc["id"].as_str().unwrap_or("?");
    let mut delivered = 0;
    for orphan in doc["orphans"].as_array().cloned().unwrap_or_default() {
        let index = orphan["index"].as_u64().unwrap_or(0);
        let why = format!("orphan body part {index} of {id}, {why_prefix}");
        let _ = unverified_part(
            state,
            this_host,
            doc["route"].as_str().unwrap_or_default(),
            doc["peer_sender"].as_str().unwrap_or_default(),
            doc["channel"].as_str().unwrap_or_default(),
            id,
            orphan["text"].as_str().unwrap_or_default(),
            &why,
        );
        delivered += 1;
    }
    delivered
}

// ---------------------------------------------------------------------------------------------
// Receiver: arrivals

/// A chunked envelope (its first post) arrived from an admitted poster.
#[allow(clippy::too_many_arguments)]
pub(super) fn chunk_envelope_in(
    state: &Path,
    this_host: &str,
    route: &str,
    peer_sender: &str,
    channel: &str,
    thread: &str,
    text: &str,
    envelope: &Value,
    declaration: &crate::model::Declaration,
) -> DataResult<RelayInResult> {
    // An unconfirmed poster's chunked post takes the usual stranger path, never assembly state.
    if !is_confirmed_sender(state, peer_sender, route, channel) {
        return relay_in_envelope(
            state,
            this_host,
            route,
            peer_sender,
            channel,
            thread,
            text,
            envelope,
            declaration,
            None,
        );
    }
    let id = envelope["id"].as_str().unwrap_or_default().to_string();
    let parts = envelope["body_parts"].as_u64().unwrap_or(0) as usize;
    let why = if !valid_message_id(&id) {
        Some("a message sent in parts with an invalid message ID".to_string())
    } else if !(2..=MAX_PARTS).contains(&parts) {
        Some(format!(
            "a message claiming {parts} parts (2 to {MAX_PARTS} allowed)"
        ))
    } else if !verify_envelope_parts(envelope) {
        Some("a message sent in parts whose parts list is not signed by its key".to_string())
    } else {
        None
    };
    if let Some(why) = why {
        return unverified(state, this_host, route, peer_sender, channel, text, &why);
    }
    let now = now_unix_ms();
    let mut late_unverified: Vec<(String, String)> = Vec::new();
    let mut set_aside = Vec::new();
    {
        let _lock = lock_store(state)?;
        let existing = load_assembly(state, &id);
        let mut keep_bound = None;
        if existing.is_none() {
            let adding = envelope["body"].as_str().map_or(0, str::len);
            (set_aside, keep_bound) = make_room(state, peer_sender, &id, adding);
        }
        let mut doc =
            existing.unwrap_or_else(|| new_assembly(&id, route, peer_sender, channel, now));
        if !doc["envelope"].is_null() {
            if doc["envelope"]["part_sha256"] != envelope["part_sha256"] {
                drop(_lock);
                return unverified(
                    state,
                    this_host,
                    route,
                    peer_sender,
                    channel,
                    text,
                    &format!("a second, different parts list for {id}"),
                );
            }
        } else {
            let mut stored = envelope.clone();
            stored["body"] = json!("");
            doc["envelope"] = stored;
            doc["to_team"] = json!(declaration.team);
            doc["thread"] = json!(thread);
            doc["post_text"] = json!(text);
            // Parts that arrived before their envelope are checked now.
            let orphans = doc["orphans"].as_array().cloned().unwrap_or_default();
            doc["orphans"] = json!([]);
            for orphan in orphans {
                let index = orphan["index"].as_u64().unwrap_or(0) as usize;
                let part_text = orphan["text"].as_str().unwrap_or_default().to_string();
                match check_part(
                    &doc,
                    index,
                    orphan["of"].as_u64().unwrap_or(0) as usize,
                    &part_text,
                ) {
                    PartCheck::Verified(restored) => {
                        doc["parts"][index.to_string()] = json!(restored);
                    }
                    PartCheck::Duplicate => {}
                    PartCheck::Unverified(why) => late_unverified.push((part_text, why)),
                }
            }
        }
        let first = envelope["body"].as_str().unwrap_or_default();
        if doc["parts"].get("1").is_none() {
            match part_hash(&doc, 1).and_then(|hash| restore_part(first, &hash)) {
                Some(restored) => doc["parts"]["1"] = json!(restored),
                None => late_unverified.push((
                    first.to_string(),
                    format!("part 1 of {id} does not match its signed hash"),
                )),
            }
        }
        doc["last_at"] = json!(now);
        if let Some(bound) = keep_bound {
            doc["force_partial"] = json!(bound);
        }
        save_assembly(state, &id, &doc)?;
    }
    for aside in &set_aside {
        deliver_marked(state, this_host, aside, now);
    }
    for (part_text, why) in late_unverified {
        let _ = unverified_part(
            state,
            this_host,
            route,
            peer_sender,
            channel,
            &id,
            &part_text,
            &why,
        );
    }
    log_event(
        state,
        &json!({"event": "relay-in", "outcome": "parts-envelope-held", "route": route, "id": id, "parts": parts}),
    );
    match progress(state, this_host, &id, now)? {
        Some(delivered) => Ok(delivered),
        None => Ok(result(
            "part-held",
            format!("part 1 of {parts} of {id} held until every part arrives"),
        )),
    }
}

/// A body-part post arrived from an admitted poster.
#[allow(clippy::too_many_arguments)]
pub(super) fn chunk_part_in(
    state: &Path,
    this_host: &str,
    route: &str,
    peer_sender: &str,
    channel: &str,
    text: &str,
    part: &Value,
) -> DataResult<RelayInResult> {
    if !is_confirmed_sender(state, peer_sender, route, channel) {
        let detail = "body-part post from an unconfirmed sender".to_string();
        record_unparseable_stranger(state, route, peer_sender, channel, text, &detail)?;
        return Ok(result(NOT_RECORDED, detail));
    }
    // Every field of a part post is untrusted until it matches a signed parts list.
    let id = part["id"].as_str().unwrap_or_default().to_string();
    let index = part["index"].as_u64().unwrap_or(0) as usize;
    let of = part["of"].as_u64().unwrap_or(0) as usize;
    let part_text = part["text"].as_str().unwrap_or_default().to_string();
    let why = if !valid_message_id(&id) {
        Some("a body part with an invalid message ID".to_string())
    } else if part["text"].as_str().is_none() || part_text.len() > MAX_PART_TEXT_BYTES {
        Some(format!(
            "a body part with no text or over {MAX_PART_TEXT_BYTES} bytes"
        ))
    } else if !(2..=MAX_PARTS).contains(&index) || !(2..=MAX_PARTS).contains(&of) || index > of {
        Some(format!("body part {index} of {of} is out of range"))
    } else {
        None
    };
    if let Some(why) = why {
        return unverified(state, this_host, route, peer_sender, channel, text, &why);
    }
    let now = now_unix_ms();
    let mut set_aside = Vec::new();
    let mut keep_bound = None;
    let outcome = {
        let _lock = lock_store(state)?;
        let existing = load_assembly(state, &id);
        if existing
            .as_ref()
            .is_none_or(|doc| doc["state"] != "complete")
        {
            (set_aside, keep_bound) = make_room(state, peer_sender, &id, part_text.len());
        }
        let mut doc =
            existing.unwrap_or_else(|| new_assembly(&id, route, peer_sender, channel, now));
        let outcome = if doc["envelope"].is_null() {
            let duplicate = doc["orphans"].as_array().is_some_and(|orphans| {
                orphans.iter().any(|orphan| {
                    orphan["index"].as_u64() == Some(index as u64)
                        && orphan["text"].as_str() == Some(part_text.as_str())
                })
            });
            if !duplicate {
                if let Some(orphans) = doc["orphans"].as_array_mut() {
                    orphans.push(json!({"index": index, "of": of, "text": part_text, "at": now}));
                }
            }
            PartCheck::Duplicate
        } else {
            let checked = check_part(&doc, index, of, &part_text);
            if let PartCheck::Verified(restored) = &checked {
                doc["parts"][index.to_string()] = json!(restored);
            }
            checked
        };
        doc["last_at"] = json!(now);
        if let Some(bound) = keep_bound {
            doc["force_partial"] = json!(bound);
        }
        save_assembly(state, &id, &doc)?;
        outcome
    };
    for aside in &set_aside {
        deliver_marked(state, this_host, aside, now);
    }
    if keep_bound.is_some() && !matches!(outcome, PartCheck::Unverified(_)) {
        deliver_marked(state, this_host, &id, now);
    }
    if let PartCheck::Unverified(why) = outcome {
        return unverified_part(
            state,
            this_host,
            route,
            peer_sender,
            channel,
            &id,
            &part_text,
            &why,
        );
    }
    match progress(state, this_host, &id, now)? {
        Some(delivered) => Ok(delivered),
        None => Ok(result(
            "part-held",
            format!("part {index} of {of} of {id} held until every part arrives"),
        )),
    }
}

enum PartCheck {
    /// The part matches its signed hash (after any Slack restoration): its text.
    Verified(String),
    /// A copy of a part already held.
    Duplicate,
    /// Delivered on its own as UNVERIFIED, for this reason.
    Unverified(String),
}

fn check_part(doc: &Value, index: usize, of: usize, text: &str) -> PartCheck {
    let id = doc["id"].as_str().unwrap_or("?");
    let parts = part_count(doc);
    if of != parts || index < 2 || index > parts {
        return PartCheck::Unverified(format!(
            "body part {index} of {of} does not fit the signed list of {parts} parts for {id}"
        ));
    }
    let Some(expected) = part_hash(doc, index) else {
        return PartCheck::Unverified(format!("no signed hash for part {index} of {id}"));
    };
    match (
        restore_part(text, &expected),
        doc["parts"].get(index.to_string()),
    ) {
        (Some(_), Some(_)) => PartCheck::Duplicate,
        (Some(restored), None) => PartCheck::Verified(restored),
        (None, Some(_)) => {
            PartCheck::Unverified(format!("a second, different text for part {index} of {id}"))
        }
        (None, None) => PartCheck::Unverified(format!(
            "part {index} of {id} does not match its signed hash"
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Receiver: deliveries

/// Deliver what an assembly is due: the whole message once every part is verified, the verified
/// parts once a part is PARTIAL_AFTER_MS late (or a bound set it aside), and after a partial
/// delivery the late parts as a second delivery. Returns the delivery's result, when one ran.
fn progress(
    state: &Path,
    this_host: &str,
    id: &str,
    now: i64,
) -> DataResult<Option<RelayInResult>> {
    let Some(doc) = load_assembly(state, id) else {
        return Ok(None);
    };
    if doc["envelope"].is_null() {
        return Ok(None);
    }
    let parts = part_count(&doc);
    let verified = verified_parts(&doc);
    let all = verified.len() == parts;
    let forced = doc["force_partial"].as_str().map(str::to_string);
    match doc["state"].as_str().unwrap_or("assembling") {
        "assembling" if all => deliver_whole(state, this_host, doc).map(Some),
        "assembling"
            if forced.is_some()
                || now - doc["first_at"].as_i64().unwrap_or(now) >= PARTIAL_AFTER_MS =>
        {
            deliver_partial(state, this_host, doc, forced.as_deref()).map(Some)
        }
        "partial" => {
            let delivered: Vec<usize> = doc["delivered_parts"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_u64)
                        .map(|n| n as usize)
                        .collect()
                })
                .unwrap_or_default();
            let late: Vec<usize> = verified
                .iter()
                .copied()
                .filter(|index| !delivered.contains(index))
                .collect();
            let quiet = now - doc["last_at"].as_i64().unwrap_or(now) >= PARTIAL_AFTER_MS;
            if !late.is_empty() && (all || quiet || forced.is_some()) {
                deliver_rest(state, this_host, doc, &late).map(Some)
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

fn declaration_for_team(state: &Path, team: &str) -> Option<crate::model::Declaration> {
    load_declarations(state)
        .ok()?
        .into_iter()
        .find(|(name, _, _)| name == team)
        .map(|(_, _, declaration)| declaration)
}

fn joined(doc: &Value, indices: &[usize], missing_line: bool) -> String {
    let parts = part_count(doc);
    let mut body = String::new();
    for index in 1..=parts {
        match doc["parts"].get(index.to_string()).and_then(Value::as_str) {
            Some(text) if indices.contains(&index) => body.push_str(text),
            _ if missing_line => {
                if !body.is_empty() && !body.ends_with('\n') {
                    body.push('\n');
                }
                body.push_str(&format!("[part {index} of {parts} missing]\n"));
            }
            _ => {}
        }
    }
    body
}

fn mark(state: &Path, id: &str, update: impl FnOnce(&mut Value)) -> DataResult<()> {
    let _lock = lock_store(state)?;
    if let Some(mut doc) = load_assembly(state, id) {
        update(&mut doc);
        save_assembly(state, id, &doc)?;
    }
    Ok(())
}

/// Every part verified: the joined body through the usual relay path (signature, tag, ledger).
fn deliver_whole(state: &Path, this_host: &str, doc: Value) -> DataResult<RelayInResult> {
    let id = doc["id"].as_str().unwrap_or_default().to_string();
    let parts = part_count(&doc);
    let all: Vec<usize> = (1..=parts).collect();
    let mut full = doc["envelope"].clone();
    full["body"] = json!(joined(&doc, &all, false));
    strip_chunk_fields(&mut full);
    mark(state, &id, |doc| {
        doc["state"] = json!("complete");
        doc["delivered_parts"] = json!(all);
        clear_texts(doc);
    })?;
    let Some(declaration) = declaration_for_team(state, doc["to_team"].as_str().unwrap_or(""))
    else {
        return Ok(result(
            "ignored-other-team",
            "no local team for this message",
        ));
    };
    log_event(
        state,
        &json!({"event": "relay-in", "outcome": "parts-joined", "id": id, "parts": parts}),
    );
    relay_in_envelope(
        state,
        this_host,
        doc["route"].as_str().unwrap_or_default(),
        doc["peer_sender"].as_str().unwrap_or_default(),
        doc["channel"].as_str().unwrap_or_default(),
        doc["thread"].as_str().unwrap_or_default(),
        doc["post_text"].as_str().unwrap_or_default(),
        &full,
        &declaration,
        None,
    )
}

/// A part still missing: the verified parts through the usual relay path, the first line naming
/// which parts were verified and which are missing.
fn deliver_partial(
    state: &Path,
    this_host: &str,
    doc: Value,
    forced: Option<&str>,
) -> DataResult<RelayInResult> {
    let id = doc["id"].as_str().unwrap_or_default().to_string();
    let parts = part_count(&doc);
    let verified = verified_parts(&doc);
    let missing: Vec<usize> = (1..=parts)
        .filter(|index| !verified.contains(index))
        .collect();
    let mut head = format!(
        "parts {} of {parts} verified; {} missing; reconfirm before acting",
        if verified.is_empty() {
            "none".to_string()
        } else {
            ranges(&verified)
        },
        ranges(&missing)
    );
    if let Some(bound) = forced {
        head = format!("delivered early, its poster reached the bound of {bound}; {head}");
    }
    let mut partial = doc["envelope"].clone();
    partial["body"] = json!(joined(&doc, &verified, true));
    strip_chunk_fields(&mut partial);
    mark(state, &id, |doc| {
        doc["state"] = json!("partial");
        doc["delivered_parts"] = json!(verified);
        if let Some(object) = doc.as_object_mut() {
            object.remove("force_partial");
        }
    })?;
    let Some(declaration) = declaration_for_team(state, doc["to_team"].as_str().unwrap_or(""))
    else {
        return Ok(result(
            "ignored-other-team",
            "no local team for this message",
        ));
    };
    log_event(
        state,
        &json!({"event": "relay-in", "outcome": "parts-partial-delivered", "id": id, "verified": verified, "missing": missing}),
    );
    relay_in_envelope(
        state,
        this_host,
        doc["route"].as_str().unwrap_or_default(),
        doc["peer_sender"].as_str().unwrap_or_default(),
        doc["channel"].as_str().unwrap_or_default(),
        doc["thread"].as_str().unwrap_or_default(),
        doc["post_text"].as_str().unwrap_or_default(),
        &partial,
        &declaration,
        Some(&head),
    )
}

/// After a partial delivery, the parts that came late: a SECOND delivery under its own key,
/// carrying the late parts and the whole-body hash verdict. Proven like the first delivery only
/// when the ledger delivered that one; otherwise it is UNVERIFIED.
fn deliver_rest(
    state: &Path,
    this_host: &str,
    doc: Value,
    late: &[usize],
) -> DataResult<RelayInResult> {
    let id = doc["id"].as_str().unwrap_or_default().to_string();
    let parts = part_count(&doc);
    let verified = verified_parts(&doc);
    let route = doc["route"].as_str().unwrap_or_default().to_string();
    let peer_sender = doc["peer_sender"].as_str().unwrap_or_default().to_string();
    let channel = doc["channel"].as_str().unwrap_or_default().to_string();
    let verdict = if verified.len() == parts {
        let whole = joined(&doc, &verified, false);
        if Some(sha256_hex(whole.as_bytes()).as_str()) == doc["envelope"]["body_sha256"].as_str() {
            format!("all {parts} parts now verified; the whole body matches its signed hash")
        } else {
            format!("all {parts} parts now verified; the whole body differs from its signed hash")
        }
    } else {
        let missing: Vec<usize> = (1..=parts)
            .filter(|index| !verified.contains(index))
            .collect();
        format!("{} still missing", ranges(&missing))
    };
    let head = format!(
        "late parts {} of {parts} of {id}; {verdict}; reconfirm before acting",
        ranges(late)
    );
    let mut delivered = doc["delivered_parts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    delivered.extend(late.iter().map(|index| json!(index)));
    mark(state, &id, |doc| {
        doc["delivered_parts"] = json!(delivered);
        if verified.len() == parts {
            doc["state"] = json!("complete");
            clear_texts(doc);
        }
    })?;
    let Some(declaration) = declaration_for_team(state, doc["to_team"].as_str().unwrap_or(""))
    else {
        return Ok(result(
            "ignored-other-team",
            "no local team for this message",
        ));
    };
    let first_delivered =
        crate::ledger::read_entry(state, &declaration.team, &id).is_some_and(|entry| {
            matches!(
                crate::ledger::entry_state(&entry),
                "accepted"
                    | "claimed"
                    | "typing"
                    | "presented"
                    | "delivered"
                    | "possibly_submitted"
            )
        });
    let body = joined(&doc, late, false);
    if !first_delivered {
        let why = format!("late parts of {id}, whose first delivery was not accepted");
        return unverified_part(
            state,
            this_host,
            &route,
            &peer_sender,
            &channel,
            &id,
            &body,
            &why,
        );
    }
    let key = format!("{id}-rest-{}", late.first().copied().unwrap_or(0));
    let mut rest = doc["envelope"].clone();
    rest["body"] = json!(body);
    strip_chunk_fields(&mut rest);
    let team_dir = state.join("teams.d").join(&declaration.team);
    let ledger_path = crate::ledger::entry_path(state, &declaration.team, &id);
    let deliverer = deliverer_for(&declaration, &team_dir, &ledger_path)?;
    let rendered = render_with_warnings(
        &rest,
        &format!("relay:{route}"),
        "parts verified against the sender's signed parts list",
        Some(&head),
        None,
    );
    let notice = json!({
        "id": key, "not_after": format_rfc3339_ms(now_unix_ms() + 86_400_000),
        "topic": "late-body-parts",
    });
    let outcome = deliverer.handover(&key, &notice, &rendered);
    log_event(
        state,
        &json!({"event": "relay-in", "outcome": "parts-rest-delivered", "id": id, "late": late, "handover": format!("{outcome:?}")}),
    );
    Ok(result(
        "accepted",
        format!("late parts {} of {parts} of {id} delivered", ranges(late)),
    ))
}

/// A completed assembly keeps each part's key (a copy re-offered by a rescan is then a known
/// duplicate) but not its text, so it holds no bytes against the bound.
fn clear_texts(doc: &mut Value) {
    if let Some(parts) = doc["parts"].as_object_mut() {
        for text in parts.values_mut() {
            *text = json!("");
        }
    }
}

fn strip_chunk_fields(envelope: &mut Value) {
    if let Some(object) = envelope.as_object_mut() {
        object.remove("body_parts");
        object.remove("part_sha256");
    }
    if let Some(relay) = envelope.get_mut("relay").and_then(Value::as_object_mut) {
        relay.remove("sig3");
    }
}

/// A post delivered on its own as UNVERIFIED.
fn unverified(
    state: &Path,
    this_host: &str,
    route: &str,
    peer_sender: &str,
    channel: &str,
    text: &str,
    why: &str,
) -> DataResult<RelayInResult> {
    let stripped = strip_provider_notes(text);
    let key = format!("unverified-{}", &sha256_hex(stripped.as_bytes())[..12]);
    deliver_raw_unverified(
        state,
        this_host,
        route,
        peer_sender,
        channel,
        &key,
        why,
        &stripped,
    )
}

/// One part's text delivered on its own as UNVERIFIED.
#[allow(clippy::too_many_arguments)]
fn unverified_part(
    state: &Path,
    this_host: &str,
    route: &str,
    peer_sender: &str,
    channel: &str,
    id: &str,
    part_text: &str,
    why: &str,
) -> DataResult<RelayInResult> {
    let key = format!(
        "unverified-{}",
        &sha256_hex(format!("{id}\n{why}\n{part_text}").as_bytes())[..12]
    );
    deliver_raw_unverified(
        state,
        this_host,
        route,
        peer_sender,
        channel,
        &key,
        why,
        part_text,
    )
}

/// Deliver what is due in every assembly (from `relay in` and `pump`): partials of messages
/// with a part PARTIAL_AFTER_MS late, late parts, orphan parts held ASSEMBLY_KEEP_MS with no
/// envelope (as UNVERIFIED), and drop records ASSEMBLY_KEEP_MS past their last activity.
pub fn deliver_due_assemblies(state: &Path, this_host: &str) -> usize {
    let now = now_unix_ms();
    let mut delivered = 0;
    for doc in all_assemblies(state) {
        let Some(id) = doc["id"].as_str().map(str::to_string) else {
            continue;
        };
        if !valid_message_id(&id) {
            continue;
        }
        let idle = now - doc["last_at"].as_i64().unwrap_or(now);
        if doc["envelope"].is_null() {
            if now - doc["first_at"].as_i64().unwrap_or(now) >= ASSEMBLY_KEEP_MS {
                delivered += deliver_orphans(
                    state,
                    this_host,
                    &doc,
                    "no signed envelope arrived within 24 hours",
                );
                remove_assembly(state, &id);
            }
            continue;
        }
        if matches!(progress(state, this_host, &id, now), Ok(Some(_))) {
            delivered += 1;
            continue;
        }
        if doc["state"] == "complete" && idle >= ASSEMBLY_KEEP_MS {
            remove_assembly(state, &id);
        } else if doc["state"] == "partial" && idle >= ASSEMBLY_KEEP_MS {
            // Parts still missing a day later arrive, if ever, as orphans.
            remove_assembly(state, &id);
        }
    }
    delivered
}

/// How many assemblies are held, and how many bytes (for `relay status`).
pub fn assembly_summary(state: &Path) -> (usize, usize) {
    let all = all_assemblies(state);
    let bytes = all.iter().map(held_bytes).sum();
    (all.len(), bytes)
}
