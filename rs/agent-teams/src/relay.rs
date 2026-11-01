//! The chat relay (design sections 10.6 and 10.7, provider-neutral): team
//! envelopes and receipts carried as chat posts in a shared channel.
//!
//! A post is a short human-readable line (the team tag and the body, so the
//! owner can read the channel), then the authoritative envelope as compact
//! JSON in a fenced block. The envelope is checked exactly as on any other
//! route (body hash, tag re-derived from the envelope), and delivery ends at
//! the same ledger `accept` uses everywhere, so a duplicated post can never
//! deliver twice. A receipt is one line plus a fenced receipt block, posted
//! in the envelope post's thread; a receipt never draws a receipt.
//!
//! Nothing here knows a chat provider. A provider only has to report the
//! poster's stable sender ID and let a bridge post into the channel; Slack,
//! Discord, or the deterministic fake all qualify.

use crate::deliver::deliverer_for;
use crate::envelope::{check_envelope, sha256_hex, team_tag};
use crate::jsonfile::{write_json_atomic, DataError, DataResult};
use crate::ledger::{accept, AcceptContext};
use crate::model::Declaration;
use crate::proof::load_declarations;
use crate::sender::{log_event, read_outbox};
use crate::timeutil::{format_rfc3339_ms, now_unix_ms};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Fence label carrying the authoritative envelope JSON.
pub const ENVELOPE_FENCE: &str = "agent-teams-envelope";
/// Fence label carrying the authoritative receipt JSON.
pub const RECEIPT_FENCE: &str = "agent-teams-receipt";
/// Fence label carrying a relay announcement (section: auto-discovery).
pub const ANNOUNCE_FENCE: &str = "agent-teams-announce";
/// Receipt line for a post `relay-in` could not record at all.
pub const NOT_RECORDED: &str = "not-recorded";

/// Undo the escaping a chat provider applies to visible text (`&gt;` and
/// friends). Providers may rewrite visible text, which is exactly why the
/// envelope, not the rendered line, is authoritative (design 10.7).
pub fn unescape_provider_text(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// Strip the note a provider rescan appends to a rescued message, plus any
/// trailing file note, so the fenced block parses as posted.
pub fn strip_provider_notes(text: &str) -> String {
    let marker = "\n\n[agentctl] This message reached the bridge through the periodic rescan";
    match text.find(marker) {
        Some(at) => text[..at].trim_end().to_string(),
        None => text.trim_end().to_string(),
    }
}

/// Render one envelope as a chat post: tag line, the body for the owner to
/// read, then the authoritative envelope in a fenced block.
pub fn render_post(envelope: &Value) -> String {
    let body = envelope.get("body").and_then(Value::as_str).unwrap_or("");
    format!(
        "{}\n{}\n```{ENVELOPE_FENCE}\n{}\n```\n",
        team_tag(envelope),
        body,
        envelope
    )
}

/// Render one receipt as a thread post: tag line, `receipt: <state>`, then
/// the authoritative receipt JSON in a fenced block.
pub fn render_receipt(envelope: &Value, receipt: &Value) -> String {
    let state = receipt.get("state").and_then(Value::as_str).unwrap_or("?");
    format!(
        "{} receipt: {}\n```{RECEIPT_FENCE}\n{}\n```\n",
        team_tag(envelope),
        state,
        receipt
    )
}

/// Render one announcement as a chat post.
pub fn render_announce(announce: &Value) -> String {
    let team = announce.get("team").and_then(Value::as_str).unwrap_or("?");
    let host = announce.get("host").and_then(Value::as_str).unwrap_or("?");
    format!(
        "[agent-teams relay announce: {team}@{host} posts as {} for repo {}]\n```{ANNOUNCE_FENCE}\n{}\n```\n",
        announce.get("sender_id").and_then(Value::as_str).unwrap_or("?"),
        announce.get("repo_id").and_then(Value::as_str).unwrap_or("?"),
        announce
    )
}

/// What one relay post carries.
#[derive(Debug, Clone, PartialEq)]
pub enum RelayPost {
    /// A team envelope (`agent-teams-message/v1`).
    Envelope(Value),
    /// A receipt for an envelope this host sent.
    Receipt(Value),
    /// A discovery announcement.
    Announce(Value),
}

fn fenced_block(text: &str, fence: &str) -> Option<String> {
    let opener = format!("```{fence}");
    let start = text.find(&opener)? + opener.len();
    let rest = &text[start..];
    let end = rest.find("\n```").or_else(|| rest.find("```"))?;
    Some(rest[..end].trim().to_string())
}

/// Parse one relay post's text (after provider unescaping and note
/// stripping). The first line of an envelope post must be the tag derived
/// from its envelope; the caller checks that.
pub fn parse_post(text: &str) -> Result<RelayPost, String> {
    let clean = strip_provider_notes(&unescape_provider_text(text));
    for (fence, make) in [
        (
            ENVELOPE_FENCE,
            RelayPost::Envelope as fn(Value) -> RelayPost,
        ),
        (RECEIPT_FENCE, RelayPost::Receipt as fn(Value) -> RelayPost),
        (
            ANNOUNCE_FENCE,
            RelayPost::Announce as fn(Value) -> RelayPost,
        ),
    ] {
        if let Some(block) = fenced_block(&clean, fence) {
            let value: Value =
                serde_json::from_str(&block).map_err(|e| format!("bad {fence} JSON: {e}"))?;
            return Ok(make(value));
        }
    }
    Err("no agent-teams fenced block".to_string())
}

/// The human tag line of a post (first line), when present.
pub fn post_tag_line(text: &str) -> String {
    let clean = strip_provider_notes(&unescape_provider_text(text));
    clean.lines().next().unwrap_or("").trim().to_string()
}

/// A repository identity for discovery: SHA-256 of the normalized origin
/// URL (scheme, credentials, trailing `.git` and slashes removed, host and
/// path lowercased). Two checkouts of one repository share an ID without
/// naming the repository anywhere.
pub fn repo_id(origin_url: &str) -> String {
    let mut url = origin_url.trim().to_lowercase();
    if let Some((_, rest)) = url.split_once("://") {
        let host_path = match rest.split_once('@') {
            Some((_, after_user)) => after_user.to_string(),
            None => rest.to_string(),
        };
        url = host_path;
    } else if let Some((host, path)) = url.split_once(':') {
        // scp-like git@host:path
        if url.contains('@') {
            let host = host.rsplit('@').next().unwrap_or(host);
            url = format!("{host}/{path}");
        }
    }
    let url = url.trim_end_matches('/').trim_end_matches(".git");
    sha256_hex(url.as_bytes())
}

/// Build one announcement value. `integrity` is a hash of the canonical
/// fields: it proves the block was not altered in transit, never identity.
/// Identity comes from the platform-authenticated poster ID equalling
/// `sender_id`, and the mapping is only a candidate until confirmed.
pub fn build_announce(team: &str, host: &str, sender_id: &str, repo: &str) -> Value {
    let mut announce = json!({
        "schema": "agent-teams-announce/v1",
        "fleet": team,
        "team": team,
        "host": host,
        "sender_id": sender_id,
        "repo_id": repo_id(repo),
        "created_at": format_rfc3339_ms(now_unix_ms()),
    });
    announce["integrity"] = json!(announce_integrity(&announce));
    announce
}

/// The relay discovery store: candidates learned from announcements and
/// confirmed sender->team mappings. Confirmed mappings come only from a
/// trusted-route check or the owner (`relay confirm`); a candidate is never
/// admitted as a peer.
pub fn store_path(state: &Path) -> PathBuf {
    state.join("relay").join("peers.json")
}

/// Load the relay discovery store (empty when absent).
pub fn load_store(state: &Path) -> Value {
    std::fs::read_to_string(store_path(state))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(
            || json!({"schema": "agent-teams-relay-peers/v1", "candidates": [], "confirmed": []}),
        )
}

/// Save the relay discovery store.
pub fn save_store(state: &Path, store: &Value) -> DataResult<()> {
    write_json_atomic(&store_path(state), store)
}

fn store_list<'a>(store: &'a Value, key: &str) -> &'a [Value] {
    store
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Record one announcement as a candidate (or a conflict when the poster is
/// already confirmed for another team). Returns the recorded outcome.
pub fn record_candidate(state: &Path, announce: &Value, poster: &str) -> DataResult<String> {
    let mut store = load_store(state);
    let sender = announce
        .get("sender_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let team = announce.get("team").and_then(Value::as_str).unwrap_or("");
    if store_list(&store, "confirmed").iter().any(|c| {
        c.get("sender_id").and_then(Value::as_str) == Some(sender)
            && c.get("team").and_then(Value::as_str) != Some(team)
    }) {
        return Ok("candidate-conflict".to_string());
    }
    let candidates = store
        .get_mut("candidates")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-store", "candidates is not a list"))?;
    let entry = json!({
        "sender_id": sender, "team": team,
        "host": announce.get("host"), "repo_id": announce.get("repo_id"),
        "poster": poster, "seen_at": format_rfc3339_ms(now_unix_ms()),
    });
    if !candidates.iter().any(|c| {
        c.get("sender_id").and_then(Value::as_str) == Some(sender)
            && c.get("team").and_then(Value::as_str) == Some(team)
            && c.get("host") == announce.get("host")
    }) {
        candidates.push(entry);
    }
    save_store(state, &store)?;
    Ok("candidate-recorded".to_string())
}

/// Confirm one sender->team mapping. `source` is `trusted-route` or `owner`.
pub fn confirm_peer(
    state: &Path,
    sender_id: &str,
    team: &str,
    host: &str,
    repo_id_value: Option<&str>,
    source: &str,
) -> DataResult<()> {
    let mut store = load_store(state);
    let confirmed = store
        .get_mut("confirmed")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-store", "confirmed is not a list"))?;
    confirmed.retain(|c| c.get("sender_id").and_then(Value::as_str) != Some(sender_id));
    confirmed.push(json!({
        "sender_id": sender_id, "team": team, "host": host,
        "repo_id": repo_id_value, "source": source,
        "confirmed_at": format_rfc3339_ms(now_unix_ms()),
    }));
    save_store(state, &store)
}

/// Fill an argv template (`{channel}`, `{thread}`, `{message_id}`,
/// `{text_file}`, `{text}`) without word splitting.
pub fn fill_argv(
    template: &[String],
    channel: &str,
    thread: &str,
    message_id: &str,
    text_file: &Path,
    text: &str,
) -> Vec<String> {
    template
        .iter()
        .map(|element| {
            element
                .replace("{channel}", channel)
                .replace("{thread}", thread)
                .replace("{message_id}", message_id)
                .replace("{text_file}", &text_file.to_string_lossy())
                .replace("{text}", text)
        })
        .collect()
}

/// Run one post command: write `text` to a private temp file, run the
/// filled argv, and return its stdout. Used for envelope posts and for
/// receipts in a thread; the argv is data, never a shell string.
pub fn run_post_argv(
    argv_template: &[String],
    channel: &str,
    thread: &str,
    message_id: &str,
    text: &str,
) -> Result<String, String> {
    if argv_template.is_empty() {
        return Err("no post argv configured".to_string());
    }
    let dir = std::env::temp_dir().join(format!("agent-teams-relay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text_file = dir.join(format!("post-{}.txt", now_unix_ms()));
    std::fs::write(&text_file, text).map_err(|e| e.to_string())?;
    let argv = fill_argv(argv_template, channel, thread, message_id, &text_file, text);
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| "no post argv configured".to_string())?;
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("post command failed to start: {e}"))?;
    let _ = std::fs::remove_file(&text_file);
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "post command exit {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The integrity hash of an announcement's canonical fields.
pub fn announce_integrity(announce: &Value) -> String {
    let material = format!(
        "{}\n{}\n{}\n{}\n{}",
        announce.get("team").and_then(Value::as_str).unwrap_or(""),
        announce.get("host").and_then(Value::as_str).unwrap_or(""),
        announce
            .get("sender_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
        announce
            .get("repo_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
        announce
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    sha256_hex(material.as_bytes())
}

/// The result of `relay-in` for one admitted relay post.
#[derive(Debug, Clone)]
pub struct RelayInResult {
    /// Machine outcome: `accepted`, a ledger state, `receipt-recorded`,
    /// `ignored-other-team`, `refused-sender-mismatch`,
    /// `refused-unlisted-peer`, `not-recorded`, `candidate-recorded`, ...
    pub outcome: String,
    /// Human detail for logs.
    pub detail: String,
    /// Receipt post text for the caller to put in the thread, when any.
    /// Receipts and ignored posts never carry one.
    pub receipt_text: Option<String>,
    /// Whether `relay-in` already posted the receipt itself.
    pub receipt_posted: bool,
}

fn result(outcome: &str, detail: impl Into<String>) -> RelayInResult {
    RelayInResult {
        outcome: outcome.to_string(),
        detail: detail.into(),
        receipt_text: None,
        receipt_posted: false,
    }
}

/// Handle one admitted relay post. The bridge has already admitted
/// `peer_sender` in this channel; this function never types anything as a
/// prompt. Envelopes for other teams or hosts are filtered here, silently:
/// no delivery, no receipt, and no agent is woken.
#[allow(clippy::too_many_arguments)]
pub fn relay_in(
    state: &Path,
    this_host: &str,
    route: &str,
    peer_sender: &str,
    channel: &str,
    thread: &str,
    text: &str,
) -> DataResult<RelayInResult> {
    let parsed = match parse_post(text) {
        Ok(parsed) => parsed,
        Err(reason) => {
            let mut out = result(NOT_RECORDED, format!("unparseable relay post: {reason}"));
            out.receipt_text = Some(format!(
                "agent-teams receipt: {NOT_RECORDED} (unparseable relay post)\n"
            ));
            return Ok(out);
        }
    };
    match parsed {
        RelayPost::Announce(announce) => {
            let claimed = announce
                .get("sender_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if claimed != peer_sender {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "refused-announce-sender-mismatch", "route": route, "peer_sender": peer_sender, "claimed_sender": claimed}),
                );
                return Ok(result(
                    "refused-announce-sender-mismatch",
                    "announcement claims a sender other than its poster",
                ));
            }
            if announce.get("integrity").and_then(Value::as_str)
                != Some(&announce_integrity(&announce))
            {
                return Ok(result(NOT_RECORDED, "announcement integrity hash mismatch"));
            }
            let outcome = record_candidate(state, &announce, peer_sender)?;
            log_event(
                state,
                &json!({"event": "relay-in", "outcome": outcome, "route": route, "peer_sender": peer_sender, "team": announce.get("team")}),
            );
            Ok(result(
                &outcome,
                "announcement recorded as a candidate; never admitted until confirmed",
            ))
        }
        RelayPost::Receipt(receipt) => {
            // A receipt never draws a receipt: record and stop.
            let id = receipt
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let receipt_state = receipt
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match read_outbox(state, &id) {
                Some(mut entry) => {
                    let mapped = crate::sender::map_receiver_state(&receipt_state);
                    if !mapped.is_empty() && mapped != "?" {
                        entry["state"] = json!(mapped);
                        let _ = write_json_atomic(&crate::sender::outbox_path(state, &id), &entry);
                    }
                    log_event(
                        state,
                        &json!({"event": "relay-in", "outcome": "receipt-recorded", "route": route, "id": id, "state": receipt_state}),
                    );
                    Ok(result(
                        "receipt-recorded",
                        format!("receipt {receipt_state} for {id}"),
                    ))
                }
                None => Ok(result("receipt-unknown", format!("no outbox entry {id}"))),
            }
        }
        RelayPost::Envelope(envelope) => {
            let to_team = envelope
                .get("to")
                .and_then(|t| t.get("team"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let to_host = envelope
                .get("to")
                .and_then(|t| t.get("host"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            // Addressed elsewhere: filter silently. No delivery, no receipt,
            // no agent woken; only a local log line (never the body).
            if to_host != this_host {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "ignored-other-team", "route": route, "id": envelope.get("id"), "to_host": to_host}),
                );
                return Ok(result(
                    "ignored-other-team",
                    format!("addressed to host {to_host}, not {this_host}"),
                ));
            }
            let declarations = load_declarations(state)?;
            let Some((_, _, declaration)) =
                declarations.iter().find(|(team, _, _)| *team == to_team)
            else {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "ignored-other-team", "route": route, "id": envelope.get("id"), "to_team": to_team}),
                );
                return Ok(result(
                    "ignored-other-team",
                    format!("no local team {to_team}"),
                ));
            };
            relay_in_envelope(
                state,
                this_host,
                route,
                peer_sender,
                channel,
                thread,
                text,
                &envelope,
                declaration,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn relay_in_envelope(
    state: &Path,
    this_host: &str,
    route: &str,
    peer_sender: &str,
    channel: &str,
    thread: &str,
    text: &str,
    envelope: &Value,
    declaration: &Declaration,
) -> DataResult<RelayInResult> {
    let id = envelope
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let entry = declaration.relay_channels.iter().find(|entry| {
        entry.route == route && (entry.channel.is_empty() || entry.channel == channel)
    });
    let Some(entry) = entry else {
        return Ok(result(
            "route-refused",
            format!(
                "team {} has no relay channel for route {route}",
                declaration.team
            ),
        ));
    };
    let from_team = envelope
        .get("from")
        .and_then(|f| f.get("team"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let from_host = envelope
        .get("from")
        .and_then(|f| f.get("host"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let to_fleet = envelope
        .get("to")
        .and_then(|t| t.get("team"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let receipt_for = |state_name: &str, detail: &str| -> RelayInResult {
        let receipt = json!({"id": id, "state": state_name, "detail": detail, "at": format_rfc3339_ms(now_unix_ms())});
        let mut out = result(state_name, detail.to_string());
        out.receipt_text = Some(render_receipt(envelope, &receipt));
        out
    };
    // A banned sender is dropped without a new alarm; the ban is the
    // owner's explicit silencing of an already-loud stranger.
    if is_banned(state, peer_sender) {
        log_event(
            state,
            &json!({"event": "relay-in", "outcome": "banned-sender", "route": route, "peer_sender": peer_sender, "id": id}),
        );
        return Ok(result(
            "banned-sender",
            format!("sender {peer_sender} is banned in this relay"),
        ));
    }
    // A channel scoped to one fleet refuses posts naming another fleet,
    // in either direction. Recorded locally; no ledger claim.
    if let Some(fleet) = &entry.fleet {
        if fleet != &from_team || fleet != to_fleet {
            log_event(
                state,
                &json!({"event": "relay-in", "outcome": "refused-fleet-scope", "route": route, "peer_sender": peer_sender, "channel_fleet": fleet, "from_team": from_team, "to_team": to_fleet, "id": id}),
            );
            let mut out = receipt_for("refused", &format!("channel is scoped to fleet {fleet}"));
            out.outcome = "refused-fleet-scope".to_string();
            return Ok(post_receipt(state, entry, channel, thread, &id, out));
        }
    }
    // Admission: the declaration's peer mapping, or a confirmed discovery
    // mapping, must name this poster for the envelope's sender team
    // (fleet and, when the mapping names one, host). Anything else
    // addressed to this team is a STRANGER event: loud, quarantined,
    // never delivered, never silently dropped.
    let mapped = entry
        .peers
        .get(peer_sender)
        .map(String::as_str)
        .or_else(|| {
            confirmed_mapping(state, peer_sender)
                .filter(|(fleet, host)| {
                    fleet.as_str() == from_team
                        && host.as_deref().is_none_or(|host| host == from_host)
                })
                .map(|_| from_team.as_str())
        });
    let admitted = match mapped {
        Some(mapped_team) => {
            let (mapped_fleet, mapped_host) = match mapped_team.split_once('@') {
                Some((fleet, host)) => (fleet, Some(host)),
                None => (mapped_team, None),
            };
            mapped_fleet == from_team && mapped_host.is_none_or(|host| host == from_host)
        }
        None => false,
    };
    if !admitted {
        let reason = if entry.peers.contains_key(peer_sender) {
            "sender-mismatch: poster is listed for another fleet/team"
        } else {
            "unconfirmed sender: not in the confirmed mapping for this channel"
        };
        record_stranger(
            state,
            declaration,
            entry,
            peer_sender,
            envelope,
            text,
            reason,
        )?;
        let mut out = receipt_for(
            "quarantined",
            "stranger: sender not confirmed; alarm raised to the coordinator",
        );
        out.outcome = "stranger-quarantined".to_string();
        return Ok(post_receipt(state, entry, channel, thread, &id, out));
    }
    // Malformed posts from an admitted sender are stranger events too:
    // loud and quarantined, so a broken or forged post is never silent.
    if post_tag_line(text) != team_tag(envelope) {
        record_stranger(
            state,
            declaration,
            entry,
            peer_sender,
            envelope,
            text,
            "malformed: tag does not match envelope",
        )?;
        let mut out = receipt_for(NOT_RECORDED, "malformed: tag does not match envelope");
        out.outcome = "stranger-quarantined".to_string();
        return Ok(post_receipt(state, entry, channel, thread, &id, out));
    }
    if let Err(reason) = check_envelope(envelope) {
        record_stranger(
            state,
            declaration,
            entry,
            peer_sender,
            envelope,
            text,
            &format!("malformed: envelope refused: {reason}"),
        )?;
        let mut out = receipt_for(NOT_RECORDED, &format!("envelope refused: {reason}"));
        out.outcome = "stranger-quarantined".to_string();
        return Ok(post_receipt(state, entry, channel, thread, &id, out));
    }
    let team_dir = state.join("teams.d").join(&declaration.team);
    let ledger_path = crate::ledger::entry_path(state, &declaration.team, &id);
    let deliverer = deliverer_for(declaration, &team_dir, &ledger_path)?;
    let proof_note =
        format!("asserted by {from_team}'s bridge (sender {peer_sender}) in channel {channel}");
    let ctx = AcceptContext {
        state,
        this_host,
        declaration,
        via: &format!("relay:{route}"),
        proof_note: &proof_note,
        relayed_by: None,
        peer_host: "",
    };
    let answer = accept(&ctx, envelope, deliverer.as_ref())?;
    let state_name = answer
        .state
        .clone()
        .unwrap_or_else(|| answer.outcome.clone());
    let mut out = receipt_for(&state_name, &answer.detail);
    out.outcome = answer.outcome.clone();
    Ok(post_receipt(state, entry, channel, thread, &id, out))
}

/// Post the receipt in the envelope post's thread when the declaration
/// names a receipt argv; otherwise leave the text for the caller.
fn post_receipt(
    _state: &Path,
    entry: &crate::model::RelayChannel,
    channel: &str,
    thread: &str,
    id: &str,
    mut out: RelayInResult,
) -> RelayInResult {
    if entry.receipt_argv.is_empty() {
        return out;
    }
    if let Some(text) = out.receipt_text.clone() {
        match run_post_argv(&entry.receipt_argv, channel, thread, id, &text) {
            Ok(_) => out.receipt_posted = true,
            Err(reason) => out.detail = format!("{} (receipt post failed: {reason})", out.detail),
        }
    }
    out
}

/// The confirmed discovery mapping for one sender, as `(fleet, host)`.
pub fn confirmed_mapping(state: &Path, sender_id: &str) -> Option<(String, Option<String>)> {
    let store = load_store(state);
    store_list(&store, "confirmed")
        .iter()
        .find(|c| c.get("sender_id").and_then(Value::as_str) == Some(sender_id))
        .map(|c| {
            let team = c
                .get("team")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let host = c
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match team.split_once('@') {
                Some((fleet, team_host)) => (fleet.to_string(), Some(team_host.to_string())),
                None if !host.is_empty() => (team, Some(host)),
                None => (team, None),
            }
        })
}

fn quarantine_path(state: &Path) -> PathBuf {
    state.join("relay").join("quarantine.json")
}

fn alarms_path(state: &Path) -> PathBuf {
    state.join("relay").join("alarms.json")
}

fn load_json_list(path: &Path, key: &str) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({"schema": "agent-teams-relay/v1", key: []}))
}

/// Quarantined posts, in arrival order.
pub fn quarantined_posts(state: &Path) -> Vec<Value> {
    let doc = load_json_list(&quarantine_path(state), "posts");
    doc.get("posts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Open and closed stranger alarms.
pub fn alarms(state: &Path) -> Vec<Value> {
    let doc = load_json_list(&alarms_path(state), "alarms");
    doc.get("alarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Whether the owner has banned this sender in the relay.
pub fn is_banned(state: &Path, sender_id: &str) -> bool {
    alarms(state).iter().any(|alarm| {
        alarm.get("sender_id").and_then(Value::as_str) == Some(sender_id)
            && alarm.get("state").and_then(Value::as_str) == Some("banned")
    })
}

/// Ban one sender: stops its alarm repeats. Its quarantined posts stay
/// quarantined (never delivered) until the owner removes the ban.
pub fn ban_sender(state: &Path, sender_id: &str) -> DataResult<()> {
    let mut doc = load_json_list(&alarms_path(state), "alarms");
    let list = doc
        .get_mut("alarms")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-alarms", "alarms is not a list"))?;
    let mut found = false;
    for alarm in list.iter_mut() {
        if alarm.get("sender_id").and_then(Value::as_str) == Some(sender_id) {
            alarm["state"] = json!("banned");
            found = true;
        }
    }
    if !found {
        list.push(json!({"sender_id": sender_id, "state": "banned", "reason": "banned by the owner", "first_at": format_rfc3339_ms(now_unix_ms())}));
    }
    write_json_atomic(&alarms_path(state), &doc)
}

/// Record a STRANGER event: quarantine the post (never delivered), open or
/// refresh the alarm for this sender, and raise it to the team's
/// coordinator at once. The alarm repeats until the sender is confirmed or
/// banned, so an unregistered bot can never mean silently missed messages.
fn record_stranger(
    state: &Path,
    declaration: &Declaration,
    entry: &crate::model::RelayChannel,
    peer_sender: &str,
    envelope: &Value,
    text: &str,
    reason: &str,
) -> DataResult<()> {
    let id = envelope
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // Quarantine (dedup by ID + sender: a replayed stranger post is one item).
    let mut quarantine = load_json_list(&quarantine_path(state), "posts");
    {
        let posts = quarantine
            .get_mut("posts")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-quarantine", "posts is not a list"))?;
        if !posts.iter().any(|post| {
            post.get("id").and_then(Value::as_str) == Some(id.as_str())
                && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender)
        }) {
            posts.push(json!({
                "id": id, "peer_sender": peer_sender, "route": entry.route,
                "channel": entry.channel, "envelope": envelope, "post": text,
                "reason": reason, "quarantined_at": format_rfc3339_ms(now_unix_ms()),
            }));
        }
    }
    write_json_atomic(&quarantine_path(state), &quarantine)?;
    // Alarm (one open alarm per sender + receiving team).
    let now_ms = now_unix_ms();
    let mut doc = load_json_list(&alarms_path(state), "alarms");
    let mut alarm_index: Option<usize> = None;
    {
        let list = doc
            .get_mut("alarms")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-alarms", "alarms is not a list"))?;
        for (index, alarm) in list.iter().enumerate() {
            if alarm.get("sender_id").and_then(Value::as_str) == Some(peer_sender)
                && alarm.get("local_team").and_then(Value::as_str)
                    == Some(declaration.team.as_str())
                && matches!(
                    alarm.get("state").and_then(Value::as_str),
                    Some("open") | Some("banned")
                )
            {
                alarm_index = Some(index);
                break;
            }
        }
        if let Some(index) = alarm_index {
            list[index]["reason"] = json!(reason);
            list[index]["interval_seconds"] = json!(entry.alarm_interval_seconds);
        } else {
            list.push(json!({
                "sender_id": peer_sender, "local_team": declaration.team,
                "claimed_from": envelope.get("from"), "reason": reason,
                "state": "open", "first_at": format_rfc3339_ms(now_ms),
                "last_raised_at": Value::Null, "raises": 0,
                "interval_seconds": entry.alarm_interval_seconds,
            }));
            alarm_index = Some(list.len() - 1);
        }
    }
    write_json_atomic(&alarms_path(state), &doc)?;
    log_event(
        state,
        &json!({"event": "stranger", "sender_id": peer_sender, "local_team": declaration.team, "id": id, "reason": reason}),
    );
    // Raise at once (repeats are `raise_due_alarms`' job).
    if let Some(index) = alarm_index {
        let _ = raise_alarm_at(state, index, now_ms);
    }
    Ok(())
}

/// Raise one alarm to its team's coordinator through that team's normal
/// deliverer: a queued notice the coordinator escalates to the owner
/// through the team's normal owner channel (the chat bridge, in owner
/// terms). The notice is plainly flagged as an escalation.
fn raise_alarm_at(state: &Path, index: usize, now_ms: i64) -> DataResult<bool> {
    let mut doc = load_json_list(&alarms_path(state), "alarms");
    let alarm = {
        let list = doc
            .get_mut("alarms")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-alarms", "alarms is not a list"))?;
        let Some(alarm) = list.get_mut(index) else {
            return Ok(false);
        };
        if alarm.get("state").and_then(Value::as_str) != Some("open") {
            return Ok(false);
        }
        alarm.clone()
    };
    let sender = alarm
        .get("sender_id")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let local_team = alarm
        .get("local_team")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let declarations = load_declarations(state)?;
    let Some((_, _, declaration)) = declarations.iter().find(|(team, _, _)| *team == local_team)
    else {
        return Ok(false);
    };
    let claimed = alarm.get("claimed_from").cloned().unwrap_or(Value::Null);
    let rendered = format!(
        "[stranger alarm: unconfirmed sender {sender} in the chat relay]\n\
         [team message: kind alarm; ESCALATE TO OWNER via this team's normal owner channel; sender {sender} claims {}@{}; reason: {}; posts quarantined, not delivered]\n\
         A bot the relay does not confirm posted to team {local_team} in the relay channel. It is quarantined, not delivered, and this alarm repeats until the sender is confirmed or banned.\n\
         Confirm: agent-teams relay confirm {sender}   Ban: agent-teams relay ban {sender}   Status: agent-teams relay status\n",
        claimed.get("team").and_then(Value::as_str).unwrap_or("?"),
        claimed.get("host").and_then(Value::as_str).unwrap_or("?"),
        alarm.get("reason").and_then(Value::as_str).unwrap_or("?"),
    );
    let notice_envelope = json!({
        "id": format!("alarm-{sender}"),
        "not_after": format_rfc3339_ms(now_ms + 86_400_000),
        "topic": "stranger-alarm",
    });
    let team_dir = state.join("teams.d").join(&declaration.team);
    let ledger_path = crate::ledger::entry_path(state, &declaration.team, "alarm");
    let deliverer = deliverer_for(declaration, &team_dir, &ledger_path)?;
    let raise_id = format!(
        "stranger-{}-{}",
        &sha256_hex(sender.as_bytes())[..12],
        now_ms
    );
    let outcome = deliverer.handover(&raise_id, &notice_envelope, &rendered);
    let (list_key, raised) = (
        "alarms",
        matches!(outcome, crate::deliver::Handover::Accepted),
    );
    let list = doc
        .get_mut(list_key)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-alarms", "alarms is not a list"))?;
    if let Some(alarm) = list.get_mut(index) {
        alarm["last_raised_at"] = json!(format_rfc3339_ms(now_ms));
        alarm["raises"] = json!(alarm.get("raises").and_then(Value::as_u64).unwrap_or(0) + 1);
    }
    write_json_atomic(&alarms_path(state), &doc)?;
    Ok(raised)
}

/// Raise every open alarm whose repeat is due. Called by `relay-in`,
/// `pump`, and `relay alarms`, so no daemon is needed: any relay activity
/// or pump run re-raises overdue stranger alarms.
pub fn raise_due_alarms(state: &Path, now_ms: i64) -> DataResult<Vec<String>> {
    let mut raised = Vec::new();
    let count = alarms(state).len();
    for index in 0..count {
        let doc = load_json_list(&alarms_path(state), "alarms");
        let Some(alarm) = doc
            .get("alarms")
            .and_then(Value::as_array)
            .and_then(|list| list.get(index))
        else {
            continue;
        };
        if alarm.get("state").and_then(Value::as_str) != Some("open") {
            continue;
        }
        let interval_ms = alarm
            .get("interval_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(1800) as i64
            * 1000;
        let last_ms = alarm
            .get("last_raised_at")
            .and_then(Value::as_str)
            .and_then(crate::timeutil::parse_rfc3339_ms)
            .unwrap_or(0);
        if last_ms == 0 || now_ms - last_ms >= interval_ms {
            let sender = alarm
                .get("sender_id")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            if raise_alarm_at(state, index, now_ms)? {
                raised.push(sender);
            }
        }
    }
    Ok(raised)
}

/// Close any open alarm for a sender (after confirmation).
pub fn close_alarm(state: &Path, sender_id: &str) -> DataResult<()> {
    let mut doc = load_json_list(&alarms_path(state), "alarms");
    if let Some(list) = doc.get_mut("alarms").and_then(Value::as_array_mut) {
        for alarm in list.iter_mut() {
            if alarm.get("sender_id").and_then(Value::as_str) == Some(sender_id)
                && alarm.get("state").and_then(Value::as_str) == Some("open")
            {
                alarm["state"] = json!("confirmed");
            }
        }
    }
    write_json_atomic(&alarms_path(state), &doc)
}

/// Deliver quarantined posts whose sender is now confirmed, in arrival
/// order, exactly once (the ledger claim is the dedupe). Returns the IDs
/// delivered (or already known). Malformed posts stay quarantined.
pub fn drain_quarantine(state: &Path, this_host: &str) -> DataResult<Vec<String>> {
    let posts = quarantined_posts(state);
    let mut delivered: Vec<String> = Vec::new();
    let mut remaining: Vec<Value> = Vec::new();
    for post in posts {
        let envelope = post.get("envelope").cloned().unwrap_or(Value::Null);
        let peer_sender = post
            .get("peer_sender")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let delivered_one = 'deliver: {
            if check_envelope(&envelope).is_err() {
                break 'deliver false;
            }
            if post
                .get("post")
                .and_then(Value::as_str)
                .is_none_or(|text| post_tag_line(text) != team_tag(&envelope))
            {
                break 'deliver false;
            }
            let to_team = envelope
                .get("to")
                .and_then(|t| t.get("team"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let declarations = load_declarations(state)?;
            let Some((_, _, declaration)) =
                declarations.iter().find(|(team, _, _)| team == to_team)
            else {
                break 'deliver false;
            };
            // Sender must now be admitted for this envelope.
            let from_team = envelope
                .get("from")
                .and_then(|f| f.get("team"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let from_host = envelope
                .get("from")
                .and_then(|f| f.get("host"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let admitted =
                declaration.relay_channels.iter().any(|entry| {
                    entry.peers.get(&peer_sender).is_some_and(|mapped| {
                        let (fleet, host) = match mapped.split_once('@') {
                            Some((fleet, host)) => (fleet, Some(host)),
                            None => (mapped.as_str(), None),
                        };
                        fleet == from_team && host.is_none_or(|host| host == from_host)
                    })
                }) || confirmed_mapping(state, &peer_sender).is_some_and(|(fleet, host)| {
                    fleet.as_str() == from_team
                        && host.as_deref().is_none_or(|host| host == from_host)
                });
            if !admitted || is_banned(state, &peer_sender) {
                break 'deliver false;
            }
            let id = envelope
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let team_dir = state.join("teams.d").join(&declaration.team);
            let ledger_path = crate::ledger::entry_path(state, &declaration.team, &id);
            let Ok(deliverer) = deliverer_for(declaration, &team_dir, &ledger_path) else {
                break 'deliver false;
            };
            let proof_note = format!("asserted by {from_team}'s bridge (sender {peer_sender}); delivered from quarantine after confirmation");
            let ctx = AcceptContext {
                state,
                this_host,
                declaration,
                via: "relay:quarantine",
                proof_note: &proof_note,
                relayed_by: None,
                peer_host: "",
            };
            match accept(&ctx, &envelope, deliverer.as_ref()) {
                Ok(answer) => {
                    delivered.push(id);
                    log_event(
                        state,
                        &json!({"event": "relay-quarantine", "outcome": answer.outcome, "id": delivered.last(), "peer_sender": peer_sender}),
                    );
                    true
                }
                Err(_) => false,
            }
        };
        if !delivered_one {
            remaining.push(post);
        }
    }
    let doc = json!({"schema": "agent-teams-relay/v1", "posts": remaining});
    write_json_atomic(&quarantine_path(state), &doc)?;
    Ok(delivered)
}

/// Teams known to the relay on this host, for `relay teams`: confirmed
/// mappings and unconfirmed candidates, optionally filtered to one repo.
pub fn known_teams(state: &Path, repo_filter: Option<&str>) -> Vec<Value> {
    let store = load_store(state);
    let mut out = Vec::new();
    for (key, confirmed) in [("confirmed", true), ("candidates", false)] {
        for item in store_list(&store, key) {
            if let Some(repo) = repo_filter {
                if item.get("repo_id").and_then(Value::as_str) != Some(repo) {
                    continue;
                }
            }
            let mut row = item.clone();
            row["confirmed"] = json!(confirmed);
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_round_trips_and_tag_is_checked() {
        let envelope = crate::envelope::build_envelope(
            "tm-sim-0123456789abcdef",
            "k",
            "sim",
            "host-b",
            1,
            "harness-ancestry",
            "tools",
            "host-a",
            "info",
            None,
            &[],
            false,
            0,
            7_200_000,
            None,
            "hello <team> & friends",
        );
        let post = render_post(&envelope);
        assert!(post.starts_with("[from sim@host-b via agent-teams"));
        // Simulate provider escaping of the visible text (& first, as a
        // provider escaping the original text would).
        let escaped = post
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let parsed = parse_post(&escaped).expect("parse escaped post");
        assert_eq!(parsed, RelayPost::Envelope(envelope.clone()));
        assert_eq!(post_tag_line(&post), team_tag(&envelope));
    }

    #[test]
    fn repo_ids_match_across_url_spellings() {
        let a = repo_id("https://github.com/Example/Repo.git");
        assert_eq!(a, repo_id("git@github.com:example/repo"));
        assert_eq!(a, repo_id("https://github.com/example/repo/"));
        assert_ne!(a, repo_id("https://github.com/example/other"));
    }

    #[test]
    fn receipt_never_carries_a_receipt() {
        let envelope = crate::envelope::build_envelope(
            "tm-sim-0123456789abcdef",
            "k",
            "sim",
            "host-b",
            1,
            "harness-ancestry",
            "tools",
            "host-a",
            "info",
            None,
            &[],
            false,
            0,
            7_200_000,
            None,
            "hi",
        );
        let text = render_receipt(
            &envelope,
            &json!({"id": "tm-sim-0123456789abcdef", "state": "accepted"}),
        );
        assert!(matches!(parse_post(&text), Ok(RelayPost::Receipt(_))));
    }
}
