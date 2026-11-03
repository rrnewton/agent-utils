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
        // Smart quotes a provider's formatting conversion substitutes.
        .replace(['\u{201c}', '\u{201d}'], "\"")
        .replace(['\u{2018}', '\u{2019}'], "'")
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

/// Default per-route cap on one relay post, in bytes. The owner's chat
/// publisher splits a post at 4,000 bytes *after* mrkdwn conversion
/// (`slack_format: converted`), and a split relay post is worse than a
/// truncated one: the first chunk has no fence and reads as an
/// unparseable stranger. A relay post is therefore never split — the
/// visible copy is truncated to keep the whole post under the route's
/// `max_post_bytes` (this default when unset), measured after escaping,
/// with the full body always in the fenced envelope.
pub const DEFAULT_MAX_POST_BYTES: usize = 3_500;

/// Length of `text` after the mrkdwn escaping the publisher applies to
/// visible text (`&` becomes `&amp;`, `<`/`>` become entities).
pub fn escaped_len(text: &str) -> usize {
    text.chars()
        .map(|ch| match ch {
            '&' => 5,
            '<' | '>' => 4,
            _ => ch.len_utf8(),
        })
        .sum()
}

/// Margin the sender-side skip test reserves for the escaped tag line
/// and its joins, which it cannot measure from the envelope bytes alone.
pub const TAG_AND_JOINS_MARGIN_BYTES: usize = 160;

/// Make body text safe as a visible preview: strip Markdown structure
/// (headings, bullets, quotes, tables, links, fences) so mrkdwn
/// conversion cannot grow it past the measured size, and so a cut can
/// never leave an unclosed fence around the envelope.
fn sanitize_preview(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let mut text = line.trim_start();
        while let Some(rest) = text.strip_prefix('#') {
            text = rest.trim_start();
        }
        if let Some(rest) = text.strip_prefix("> ") {
            text = rest;
        }
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = text.strip_prefix(marker) {
                text = rest;
                break;
            }
        }
        let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 {
            if let Some(rest) = text[digits..].strip_prefix(". ") {
                text = rest;
            }
        }
        out.push_str(&text.replace('|', "/").replace("```", ""));
        out.push('\n');
    }
    // Links: keep only the label, never the URL the converter expands.
    let mut linked = String::new();
    let mut rest = out.as_str();
    while let Some(open) = rest.find('[') {
        let Some(close_rel) = rest[open..].find("](") else {
            break;
        };
        let close = open + close_rel;
        let Some(end_rel) = rest[close..].find(')') else {
            break;
        };
        linked.push_str(&rest[..open]);
        linked.push_str(&rest[open + 1..close]);
        rest = &rest[close + end_rel + 1..];
    }
    linked.push_str(rest);
    linked.trim_end().to_string()
}

/// Serialize an envelope for the fence with the characters Slack acts
/// on inside string values JSON-escaped (`.`, `:`, `@`, `<`, `>`, `&`
/// become `\uXXXX`). Slack auto-links bare domain-like tokens (a file
/// named `cli.rs`, a URL, an email) even inside a code block, rewriting
/// the body so its hash fails; the escaped form parses to the identical
/// envelope, so hash and signature still verify, and Slack sees nothing
/// to link (#794). Costs 5 extra bytes per escaped character, which the
/// post-size measurement includes because it measures this fence.
pub fn fence_json(envelope: &Value) -> String {
    let raw = envelope.to_string();
    let mut out = String::with_capacity(raw.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if in_string && !escaped {
            match ch {
                '"' => {
                    in_string = false;
                    out.push(ch);
                }
                '\\' => {
                    escaped = true;
                    out.push(ch);
                }
                '.' => out.push_str("\\u002e"),
                ':' => out.push_str("\\u003a"),
                '@' => out.push_str("\\u0040"),
                '<' => out.push_str("\\u003c"),
                '>' => out.push_str("\\u003e"),
                '&' => out.push_str("\\u0026"),
                _ => out.push(ch),
            }
            continue;
        }
        if escaped {
            escaped = false;
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = true;
        }
        out.push(ch);
    }
    out
}

fn envelope_fence(envelope: &Value) -> String {
    format!("```{ENVELOPE_FENCE}\n{}\n```\n", fence_json(envelope))
}

/// Undo Slack link markup in a received body: `<http://LABEL|LABEL>`
/// (and https/mailto forms) becomes LABEL, `<URL>` becomes URL. Only
/// used to rescue a body whose hash then verifies (#794).
pub fn undo_slack_link_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &rest[open + 1..open + close];
        let replacement: Option<String> = if let Some((url, label)) = inner.split_once('|') {
            if url == format!("http://{label}")
                || url == format!("https://{label}")
                || url == format!("mailto:{label}")
                || url == label
            {
                Some(label.to_string())
            } else {
                None
            }
        } else if inner.starts_with("http://") || inner.starts_with("https://") {
            Some(inner.to_string())
        } else {
            inner.strip_prefix("mailto:").map(str::to_string)
        };
        match replacement {
            Some(text) => out.push_str(&text),
            None => out.push_str(&rest[open..=open + close]),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// The size a rendered post occupies after the publisher's escaping:
/// visible part escaped, fence raw.
pub fn measured_post_bytes(text: &str) -> usize {
    match text.find("```agent-teams-envelope") {
        Some(at) => escaped_len(&text[..at]) + text[at..].len(),
        None => escaped_len(text),
    }
}

/// Bytes the fence adds around a serialized envelope.
pub fn fence_overhead() -> usize {
    "```agent-teams-envelope\n".len() + "\n```\n".len()
}

/// Render one envelope as a chat post: tag line, the body for the owner
/// to read, then the authoritative envelope in a fenced block. The
/// visible body is truncated when it would push the post past
/// `max_post_bytes` measured after escaping; the envelope is never
/// touched. Callers skip the route when the envelope alone does not fit.
pub fn render_post_with_limit(envelope: &Value, max_post_bytes: usize) -> String {
    let raw_body = envelope.get("body").and_then(Value::as_str).unwrap_or("");
    let body = sanitize_preview(raw_body);
    let body = body.as_str();
    let tag = team_tag(envelope);
    let fence = envelope_fence(envelope);
    // Tag and visible text are escaped by the publisher; the fence is not.
    // The two `\n` joins become blank-line joins after conversion, so
    // count 2 extra bytes of join margin.
    let fits = |visible: &str| {
        escaped_len(&tag) + 1 + escaped_len(visible) + 1 + fence.len() + 2 <= max_post_bytes
    };
    const NOTE: &str = "\n[relay copy truncated: full text in the envelope below]";
    // The visible copy is only a preview for the owner: the envelope
    // already carries the full body, so a long body shows its first
    // ~500 chars and a post stays ~1x the body, not ~2x.
    const PREVIEW_CHARS: usize = 500;
    if body.chars().count() > PREVIEW_CHARS {
        let preview: String = body.chars().take(PREVIEW_CHARS).collect();
        let visible = format!("{preview}{NOTE}");
        if fits(&visible) {
            return format!("{tag}\n{visible}\n{fence}");
        }
    } else if fits(body) {
        return format!("{tag}\n{body}\n{fence}");
    }
    if !fits("") {
        return format!("{tag}\n\n{fence}");
    }
    let mut kept = String::new();
    'lines: for line in body.split_inclusive('\n') {
        if fits(&format!("{kept}{line}{NOTE}")) {
            kept.push_str(line);
            continue;
        }
        for ch in line.chars() {
            if !fits(&format!("{kept}{ch}{NOTE}")) {
                break 'lines;
            }
            kept.push(ch);
        }
        break;
    }
    format!("{tag}\n{kept}{NOTE}\n{fence}")
}

/// Render one envelope as a chat post at the default cap.
pub fn render_post(envelope: &Value) -> String {
    render_post_with_limit(envelope, DEFAULT_MAX_POST_BYTES)
}

/// Render one receipt as a thread post: tag line, `receipt: <state>`, then
/// the authoritative receipt JSON in a fenced block. The header names
/// the receipt's real direction: from the receiving side, to the
/// original sender, keeping the original message ID so it threads and
/// the bracketed-prefix tag check on inbound receipts still matches.
pub fn render_receipt(envelope: &Value, receipt: &Value) -> String {
    let state = receipt.get("state").and_then(Value::as_str).unwrap_or("?");
    let mut header_envelope = envelope.clone();
    header_envelope["from"] = envelope.get("to").cloned().unwrap_or(Value::Null);
    header_envelope["to"] = envelope.get("from").cloned().unwrap_or(Value::Null);
    format!(
        "{} receipt: {}\n```{RECEIPT_FENCE}\n{}\n```\n",
        team_tag(&header_envelope),
        state,
        fence_json(receipt)
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
        fence_json(announce)
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

/// Parse one relay post's text. The raw text is tried first — a provider
/// that already unescaped its delivery must not be unescaped twice, or a
/// body containing a literal `&lt;` would fail its hash — and the
/// provider-escaped form is the fallback for providers that escape.
pub fn parse_post(text: &str) -> Result<RelayPost, String> {
    let raw = parse_post_clean(&strip_provider_notes(text));
    match raw {
        // An envelope whose body hash fails raw was probably escaped by
        // the provider; the escaped form is authoritative in that case.
        Ok(RelayPost::Envelope(ref envelope)) if check_envelope(envelope).is_err() => {
            match parse_post_clean(&strip_provider_notes(&unescape_provider_text(text))) {
                Ok(unescaped) => Ok(unescaped),
                Err(_) => raw,
            }
        }
        other => other
            .or_else(|_| parse_post_clean(&strip_provider_notes(&unescape_provider_text(text)))),
    }
}

/// Classify one parsed JSON object by its schema, never by fence label:
/// providers strip fence info strings, so content is the only authority.
fn classify_value(value: Value) -> Option<RelayPost> {
    match value.get("schema").and_then(Value::as_str) {
        Some("agent-teams-message/v1") => Some(RelayPost::Envelope(value)),
        Some("agent-teams-announce/v1") => Some(RelayPost::Announce(value)),
        Some("agent-teams-receipt/v1") => Some(RelayPost::Receipt(value)),
        _ => {
            // Receipts posted before they carried a schema field: an
            // object with an id and a state and no sender is a receipt.
            if value.get("id").is_some()
                && value.get("state").is_some()
                && value.get("from").is_none()
                && value.get("schema").is_none()
            {
                Some(RelayPost::Receipt(value))
            } else {
                None
            }
        }
    }
}

/// Every fenced block's content, whatever its info string (including a
/// bare ``` fence), plus the whole text as a bare-JSON candidate.
fn json_candidates(clean: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = clean;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("```") else { break };
        let block = &after[..end];
        let block = block.trim();
        // Drop an info-string line when present (any label, or none).
        let block = match block.find('\n') {
            Some(nl)
                if !block[..nl].contains('{') && !block[..nl].contains(char::is_whitespace) =>
            {
                block[nl + 1..].trim()
            }
            _ => block,
        };
        if block.starts_with('{') {
            out.push(block.to_string());
        }
        rest = &after[end + 3..];
    }
    out.push(clean.trim().to_string());
    out
}

fn parse_post_clean(clean: &str) -> Result<RelayPost, String> {
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
        if let Some(block) = fenced_block(clean, fence) {
            let value: Value =
                serde_json::from_str(&block).map_err(|e| format!("bad {fence} JSON: {e}"))?;
            return Ok(make(value));
        }
    }
    // Content-based fallback: a fenced (any label, or bare) or bare JSON
    // object classified by its schema. This is what survives a provider
    // stripping the fence info string.
    for candidate in json_candidates(clean) {
        for (at, _) in candidate.match_indices('{') {
            let mut stream =
                serde_json::Deserializer::from_str(&candidate[at..]).into_iter::<Value>();
            if let Some(Ok(value)) = stream.next() {
                if let Some(post) = classify_value(value) {
                    return Ok(post);
                }
            }
        }
    }
    Err("no agent-teams fenced block".to_string())
}

/// Whether a post's visible tag agrees with its envelope. Only the
/// bracketed `[from ... · to: ... · id: ...]` prefix is compared:
/// providers collapse the newline after the tag (Slack stores tag and
/// body on one line), and whatever follows the bracket is body, not
/// tag. The signed envelope remains authoritative; this advisory
/// cross-check stays loud only when the bracket itself disagrees.
fn tag_matches(text: &str, envelope: &Value) -> bool {
    let line = post_tag_line(text);
    let line = line.trim_start();
    if !line.starts_with('[') {
        return false;
    }
    match line.find(']') {
        Some(end) => line[..=end] == team_tag(envelope),
        None => false,
    }
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

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn key_path(state: &Path, team: &str, host: &str) -> PathBuf {
    state
        .join("relay")
        .join("keys")
        .join(format!("{team}@{host}.json"))
}

/// This host's signing key for one local team@host, creating it (mode
/// 0600, never posted) on first use. Returns `(seed_hex, public_hex)`.
pub fn ensure_signing_key(state: &Path, team: &str, host: &str) -> DataResult<(String, String)> {
    let path = key_path(state, team, host);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            let seed = value["seed"].as_str().unwrap_or("").to_string();
            let public = value["public"].as_str().unwrap_or("").to_string();
            if !seed.is_empty() && !public.is_empty() {
                return Ok((seed, public));
            }
        }
    }
    let mut seed = [0u8; 32];
    {
        use std::io::Read as _;
        let mut file = std::fs::File::open("/dev/urandom")
            .map_err(|e| DataError::new("relay-key", e.to_string()))?;
        file.read_exact(&mut seed)
            .map_err(|e| DataError::new("relay-key", e.to_string()))?;
    }
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let public = bytes_hex(signing.verifying_key().as_bytes());
    let seed_hex = bytes_hex(&seed);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_json_atomic(
        &path,
        &json!({"team": team, "host": host, "seed": seed_hex, "public": public}),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok((seed_hex, public))
}

/// The local public key for a team@host, when a signing key exists.
pub fn own_public_key(state: &Path, team: &str, host: &str) -> Option<String> {
    let text = std::fs::read_to_string(key_path(state, team, host)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value["public"].as_str().map(str::to_string)
}

fn envelope_material(envelope: &Value) -> String {
    let get = |path: &[&str]| -> String {
        let mut value = envelope;
        for key in path {
            value = value.get(key).unwrap_or(&Value::Null);
        }
        value.as_str().unwrap_or("").to_string()
    };
    format!(
        "{}\n{}@{}\n{}@{}\n{}\n{}\n{}",
        get(&["id"]),
        get(&["from", "team"]),
        get(&["from", "host"]),
        get(&["to", "team"]),
        get(&["to", "host"]),
        get(&["body_sha256"]),
        get(&["created_at"]),
        get(&["not_after"]),
    )
}

/// Sign one envelope in place with its sender team@host's key.
pub fn sign_envelope(state: &Path, envelope: &mut Value) -> DataResult<()> {
    let team = envelope["from"]["team"].as_str().unwrap_or("").to_string();
    let host = envelope["from"]["host"].as_str().unwrap_or("").to_string();
    let (seed_hex, public) = ensure_signing_key(state, &team, &host)?;
    let seed = hex_bytes(&seed_hex)
        .ok_or_else(|| DataError::new("relay-key", "stored seed is not hex"))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| DataError::new("relay-key", "stored seed has a bad length"))?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    use ed25519_dalek::Signer as _;
    let signature = signing.sign(envelope_material(envelope).as_bytes());
    envelope["relay"] = json!({"pubkey": public, "sig": bytes_hex(&signature.to_bytes())});
    Ok(())
}

/// Verify an envelope's signature against its embedded public key.
pub fn verify_envelope(envelope: &Value) -> bool {
    let Some(public) = envelope["relay"]["pubkey"].as_str() else {
        return false;
    };
    let Some(sig) = envelope["relay"]["sig"].as_str() else {
        return false;
    };
    let (Some(public), Some(sig)) = (hex_bytes(public), hex_bytes(sig)) else {
        return false;
    };
    let (Ok(public), Ok(sig)) = (
        <[u8; 32]>::try_from(public.as_slice()),
        <[u8; 64]>::try_from(sig.as_slice()),
    ) else {
        return false;
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let signature = ed25519_dalek::Signature::from_bytes(&sig);
    use ed25519_dalek::Verifier as _;
    key.verify(envelope_material(envelope).as_bytes(), &signature)
        .is_ok()
}

fn receipt_material(id: &str, state_name: &str, team: &str, host: &str) -> String {
    format!("{id}\n{state_name}\n{team}@{host}")
}

/// Sign one receipt in place with the receiving team@host's key.
pub fn sign_receipt(state: &Path, receipt: &mut Value, team: &str, host: &str) -> DataResult<()> {
    let (seed_hex, public) = ensure_signing_key(state, team, host)?;
    let seed = hex_bytes(&seed_hex)
        .ok_or_else(|| DataError::new("relay-key", "stored seed is not hex"))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| DataError::new("relay-key", "stored seed has a bad length"))?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    use ed25519_dalek::Signer as _;
    let signature = signing.sign(
        receipt_material(
            receipt["id"].as_str().unwrap_or(""),
            receipt["state"].as_str().unwrap_or(""),
            team,
            host,
        )
        .as_bytes(),
    );
    receipt["relay"] = json!({"pubkey": public, "sig": bytes_hex(&signature.to_bytes())});
    Ok(())
}

/// Verify a receipt's signature against its embedded public key.
pub fn verify_receipt(receipt: &Value, team: &str, host: &str) -> bool {
    let (Some(public), Some(sig)) = (
        receipt["relay"]["pubkey"].as_str(),
        receipt["relay"]["sig"].as_str(),
    ) else {
        return false;
    };
    let (Some(public), Some(sig)) = (hex_bytes(public), hex_bytes(sig)) else {
        return false;
    };
    let (Ok(public), Ok(sig)) = (
        <[u8; 32]>::try_from(public.as_slice()),
        <[u8; 64]>::try_from(sig.as_slice()),
    ) else {
        return false;
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let signature = ed25519_dalek::Signature::from_bytes(&sig);
    use ed25519_dalek::Verifier as _;
    key.verify(
        receipt_material(
            receipt["id"].as_str().unwrap_or(""),
            receipt["state"].as_str().unwrap_or(""),
            team,
            host,
        )
        .as_bytes(),
        &signature,
    )
    .is_ok()
}

/// The trusted public key for one team@host, confirmed from its announce.
pub fn trusted_key(state: &Path, team: &str, host: &str) -> Option<String> {
    let store = load_store(state);
    store_list(&store, "keys").iter().find_map(|entry| {
        if entry.get("team").and_then(Value::as_str) == Some(team)
            && entry.get("host").and_then(Value::as_str) == Some(host)
        {
            entry
                .get("pubkey")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        }
    })
}

/// Every trusted key record (team, host, pubkey, source, time).
pub fn trusted_keys(state: &Path) -> Vec<Value> {
    let store = load_store(state);
    store_list(&store, "keys").to_vec()
}

/// Remove the trusted key for one team@host. Returns whether one existed.
pub fn untrust_key(state: &Path, team: &str, host: &str) -> DataResult<bool> {
    let _lock = lock_store(state)?;
    let mut store = load_store(state);
    let Some(keys) = store.get_mut("keys").and_then(Value::as_array_mut) else {
        return Ok(false);
    };
    let before = keys.len();
    keys.retain(|entry| {
        !(entry.get("team").and_then(Value::as_str) == Some(team)
            && entry.get("host").and_then(Value::as_str) == Some(host))
    });
    let removed = keys.len() != before;
    if removed {
        save_store(state, &store)?;
    }
    Ok(removed)
}

/// Close open alarms whose claimed_from is one team@host (after that
/// team@host's key is trusted): keyed by the claim, not the sender, so
/// a shared bot's other teams keep their alarms.
pub fn close_alarms_claiming(state: &Path, team: &str, host: &str) -> DataResult<()> {
    let _lock = lock_store(state)?;
    let mut doc = load_json_list(&alarms_path(state), "alarms");
    if let Some(list) = doc.get_mut("alarms").and_then(Value::as_array_mut) {
        for alarm in list.iter_mut() {
            let claimed = alarm.get("claimed_from");
            if claimed.and_then(|c| c.get("team")).and_then(Value::as_str) == Some(team)
                && claimed.and_then(|c| c.get("host")).and_then(Value::as_str) == Some(host)
                && alarm.get("state").and_then(Value::as_str) == Some("open")
            {
                alarm["state"] = json!("confirmed");
            }
        }
    }
    write_json_atomic(&alarms_path(state), &doc)
}

/// Record a confirmed public key for one team@host.
pub fn trust_key(
    state: &Path,
    team: &str,
    host: &str,
    pubkey: &str,
    source: &str,
) -> DataResult<()> {
    let _lock = lock_store(state)?;
    let mut store = load_store(state);
    if store.get("keys").is_none() {
        store["keys"] = json!([]);
    }
    let keys = store
        .get_mut("keys")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-store", "keys is not a list"))?;
    keys.retain(|entry| {
        !(entry.get("team").and_then(Value::as_str) == Some(team)
            && entry.get("host").and_then(Value::as_str) == Some(host))
    });
    keys.push(json!({
        "team": team, "host": host, "pubkey": pubkey, "source": source,
        "confirmed_at": format_rfc3339_ms(now_unix_ms()),
    }));
    save_store(state, &store)
}

/// The relay discovery store: candidates learned from announcements and
/// confirmed sender->team mappings. Confirmed mappings come only from a
/// trusted-route check or the owner (`relay confirm`); a candidate is never
/// admitted as a peer.
pub fn store_path(state: &Path) -> PathBuf {
    state.join("relay").join("peers.json")
}

/// Take the relay store lock: every read-modify-write of `peers.json`,
/// `alarms.json` and `quarantine.json` holds it, so a ban or confirm can
/// never be overwritten by a concurrent raise or drain.
fn lock_store(state: &Path) -> DataResult<std::fs::File> {
    use fs2::FileExt;
    let dir = state.join("relay");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join(".store.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
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
pub fn record_candidate(
    state: &Path,
    announce: &Value,
    poster: &str,
    route: &str,
) -> DataResult<String> {
    let _lock = lock_store(state)?;
    let mut store = load_store(state);
    let sender = announce
        .get("sender_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let team = announce.get("team").and_then(Value::as_str).unwrap_or("");
    let host = announce.get("host").and_then(Value::as_str).unwrap_or("");
    if store_list(&store, "confirmed").iter().any(|c| {
        c.get("sender_id").and_then(Value::as_str) == Some(sender)
            && (c.get("team").and_then(Value::as_str) != Some(team)
                || c.get("host").and_then(Value::as_str) != Some(host))
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
        "pubkey": announce.get("pubkey"),
        "poster": poster, "route": route, "seen_at": format_rfc3339_ms(now_unix_ms()),
    });
    if !candidates.iter().any(|c| {
        c.get("sender_id").and_then(Value::as_str) == Some(sender)
            && c.get("team").and_then(Value::as_str) == Some(team)
            && c.get("host") == announce.get("host")
    }) {
        candidates.push(entry);
        // Candidates are claims, not a growing collection to keep forever:
        // keep the newest 256.
        if candidates.len() > 256 {
            let excess = candidates.len() - 256;
            candidates.drain(0..excess);
        }
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
    route: Option<&str>,
) -> DataResult<()> {
    let _lock = lock_store(state)?;
    let mut store = load_store(state);
    // One bot per team is a hard requirement: a sender ID already confirmed
    // for a different team or host is a conflict, never a silent replacement.
    if let Some(existing) = store_list(&store, "confirmed")
        .iter()
        .find(|c| c.get("sender_id").and_then(Value::as_str) == Some(sender_id))
    {
        let old_team = existing.get("team").and_then(Value::as_str).unwrap_or("");
        let old_host = existing.get("host").and_then(Value::as_str).unwrap_or("");
        if old_team != team || old_host != host {
            return Err(DataError::new(
                "relay-confirm-conflict",
                format!(
                    "sender {sender_id} is already confirmed for {old_team}@{old_host}; \
                     refusing to silently remap it to {team}@{host} (one bot per team)"
                ),
            ));
        }
    }
    let confirmed = store
        .get_mut("confirmed")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-store", "confirmed is not a list"))?;
    confirmed.retain(|c| c.get("sender_id").and_then(Value::as_str) != Some(sender_id));
    confirmed.push(json!({
        "sender_id": sender_id, "team": team, "host": host,
        "repo_id": repo_id_value, "source": source, "route": route,
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
    run_post_argv_with_timeout(
        argv_template,
        channel,
        thread,
        message_id,
        text,
        POST_TIMEOUT_SECONDS,
    )
}

/// [`run_post_argv`] with a caller-chosen timeout (a relay route's
/// `post_timeout_seconds`).
pub fn run_post_argv_with_timeout(
    argv_template: &[String],
    channel: &str,
    thread: &str,
    message_id: &str,
    text: &str,
    timeout_seconds: u64,
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
    let output = run_with_timeout(program, args, timeout_seconds);
    let _ = std::fs::remove_file(&text_file);
    let output = output?;
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

/// Default for how long a post command may run. A chat post is one API
/// call (live posts take about 2 s); anything slower is hung or waiting
/// out a rate limit, and a hung post must not stall a `send` (or `pump`,
/// holding `pump.lock`) behind it. A relay route may raise it with
/// `post_timeout_seconds` when its helper waits out Slack `retry_after`.
pub const POST_TIMEOUT_SECONDS: u64 = 10;

/// Run a post command with a timeout, in its own process group. On
/// timeout the whole GROUP is killed — the helper is normally a shell
/// that execs another tool, and killing only the shell would leave the
/// tool running to finish the post anyway. Output goes to temp files,
/// never pipes: a child (or a grandchild holding the pipes) can then
/// neither block on a full pipe nor keep the caller waiting for EOF
/// after it exits. The error on timeout says so (never "failed to
/// start", so callers classify it as an uncertain post, not a definite
/// non-delivery).
fn run_with_timeout(
    program: &str,
    args: &[String],
    timeout_seconds: u64,
) -> Result<std::process::Output, String> {
    use std::os::unix::process::CommandExt;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = format!(
        "agent-teams-post-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    // Temp files live in the per-process relay dir, created new with
    // owner-only permissions: a failure to create one means the post
    // never started, a definite non-delivery, and says so.
    let dir = std::env::temp_dir().join(format!("agent-teams-relay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("post command failed to start: {e}"))?;
    let out_path = dir.join(format!("{stamp}.out"));
    let err_path = dir.join(format!("{stamp}.err"));
    let temp_file = |path: &std::path::Path| -> Result<std::fs::File, String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)
            .map_err(|e| format!("post command failed to start: {e}"))
    };
    let out_file = temp_file(&out_path)?;
    let err_file = match temp_file(&err_path) {
        Ok(file) => file,
        Err(error) => {
            let _ = std::fs::remove_file(&out_path);
            return Err(error);
        }
    };
    let spawned = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(out_file)
        .stderr(err_file)
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&out_path);
            let _ = std::fs::remove_file(&err_path);
            return Err(format!("post command failed to start: {e}"));
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_seconds);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                // Kill the process group (negative PID), not just the shell.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                break child
                    .wait()
                    .map_err(|e| e.to_string())
                    .and_then(|_| Err(format!("post command timed out after {timeout_seconds}s")));
            }
            Err(e) => break Err(e.to_string()),
        }
    };
    let stdout = std::fs::read(&out_path).unwrap_or_default();
    let stderr = std::fs::read(&err_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&err_path);
    Ok(std::process::Output {
        status: status?,
        stdout,
        stderr,
    })
}

/// The integrity hash of an announcement's canonical fields.
pub fn announce_integrity(announce: &Value) -> String {
    let material = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
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
        announce.get("pubkey").and_then(Value::as_str).unwrap_or(""),
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
    in_thread: bool,
    text: &str,
) -> DataResult<RelayInResult> {
    // Our own plain-text unparseable receipt is itself unparseable: never
    // quarantine it, or the two sides ping-pong receipts forever (#763).
    if strip_provider_notes(text).trim()
        == "agent-teams receipt: not-recorded (unparseable relay post)"
    {
        return Ok(result(
            "ignored-unparseable-receipt",
            "plain-text unparseable receipt; ignored without a reply",
        ));
    }
    let parsed = match parse_post(text) {
        Ok(parsed) => parsed,
        Err(reason) => {
            // A post from a CONFIRMED sender that does not parse (a split
            // or truncated relay post is the usual cause) is not a
            // stranger: it is quarantined under its own alarm kind.
            // Never reply to an unparseable post (its receipt would itself
            // be unparseable), so no receipt_text is set on either branch.
            if is_confirmed_sender(state, peer_sender, route, channel) {
                let detail = format!("unparseable relay post from confirmed sender: {reason}");
                let id = record_unparseable_confirmed(
                    state,
                    route,
                    peer_sender,
                    channel,
                    text,
                    &detail,
                )?;
                return Ok(result(
                    NOT_RECORDED,
                    format!(
                        "{detail}; quarantined as {id} — dismiss with: \
                         agent-teams relay quarantine dismiss {id}"
                    ),
                ));
            }
            // Unparseable posts from anyone else are stranger events:
            // quarantine and alarm, never silently dropped. Provider
            // formatting, splitting or truncation lands here. A recording
            // failure is an admission failure, never a silent drop.
            let detail = format!("unparseable relay post: {reason}");
            record_unparseable_stranger(state, route, peer_sender, channel, text, &detail)?;
            return Ok(result(NOT_RECORDED, detail));
        }
    };
    match parsed {
        RelayPost::Announce(announce) => {
            // Our own announce loops back through our bridge: recognized
            // by its team@host key and ignored, never a stranger.
            let announce_team = announce.get("team").and_then(Value::as_str).unwrap_or("");
            let announce_host = announce.get("host").and_then(Value::as_str).unwrap_or("");
            let own = own_public_key(state, announce_team, announce_host);
            // "Own" means the announcing team itself owns this relay route
            // in its local declaration. Another local team's key material
            // in a shared state dir (the simulated-peer case) is not own.
            let owns_route = load_declarations(state).is_ok_and(|declarations| {
                declarations.iter().any(|(team, _, declaration)| {
                    team == announce_team
                        && declaration.relay_channels.iter().any(|entry| {
                            entry.route == route
                                && (entry.channel.is_empty() || entry.channel == channel)
                        })
                })
            });
            if announce_host == this_host
                && owns_route
                && own.is_some()
                && own.as_deref() == announce.get("pubkey").and_then(Value::as_str)
            {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "ignored-own-announce", "route": route, "peer_sender": peer_sender}),
                );
                return Ok(result(
                    "ignored-own-announce",
                    "signed announce from our own team",
                ));
            }
            let claimed = announce
                .get("sender_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if claimed != peer_sender {
                record_unparseable_stranger(
                    state,
                    route,
                    peer_sender,
                    channel,
                    text,
                    "announcement claims a sender other than its poster",
                )?;
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
                record_unparseable_stranger(
                    state,
                    route,
                    peer_sender,
                    channel,
                    text,
                    "announcement integrity hash mismatch",
                )?;
                return Ok(result(NOT_RECORDED, "announcement integrity hash mismatch"));
            }
            let outcome = record_candidate(state, &announce, peer_sender, route)?;
            if outcome == "candidate-conflict" {
                // A second team claim for one confirmed sender ID is a
                // conflict alarm, never a silent replacement.
                record_unparseable_stranger(
                    state,
                    route,
                    peer_sender,
                    channel,
                    text,
                    "conflict: sender already confirmed for another team (one bot per team)",
                )?;
            }
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
            // A receipt never draws a receipt: record and stop. Receipts are
            // authenticated like envelopes: the ID must be a well-formed
            // message ID (never a path), the poster must be mapped to the
            // fleet the outbox entry was sent to, banned senders are
            // dropped, the state must be a known ledger state, and a
            // terminal outbox state is never rewritten by a replay.
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
            const KNOWN_STATES: &[&str] = &[
                "accepted",
                "delivered",
                "refused",
                "expired",
                "superseded",
                "retracted",
                "fenced",
                "claimed",
                "typing",
                "presented",
                "in-custody",
                // A receiver whose delivery target is missing or archived
                // answers `deferred` (#721 agent-teams-coordinator-handover;
                // the first build of that said `undeliverable`): nothing was
                // claimed there, so the message must become retryable here.
                "deferred",
                "undeliverable",
            ];
            if !crate::envelope::valid_message_id(&id)
                || !KNOWN_STATES.contains(&receipt_state.as_str())
            {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "receipt-refused", "route": route, "peer_sender": peer_sender, "detail": "bad id or state"}),
                );
                return Ok(result("receipt-refused", "receipt has a bad ID or state"));
            }
            if is_banned(state, peer_sender) {
                return Ok(result("banned-sender", "banned sender's receipt dropped"));
            }
            // A receipt must arrive in the original post's thread: a
            // top-level channel post never settles anything. Providers
            // synthesise a thread ID for top-level posts too, so the
            // bridge's thread-reply flag, not the ID, decides.
            if thread.is_empty() || !in_thread {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "receipt-refused", "route": route, "peer_sender": peer_sender, "id": id, "detail": "receipt not in a thread"}),
                );
                return Ok(result(
                    "receipt-refused",
                    "receipt did not arrive in the original post's thread",
                ));
            }
            let Some(mut entry) = read_outbox(state, &id) else {
                return Ok(result("receipt-unknown", format!("no outbox entry {id}")));
            };
            // The poster must speak for the exact team@host this message
            // was sent to, confirmed on this route (never another route,
            // fleet-only, or a different host of the same fleet).
            let to_fleet = entry["envelope"]["to"]["team"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let to_host = entry["envelope"]["to"]["host"]
                .as_str()
                .unwrap_or("")
                .to_string();
            // Signed mode: a trusted key for the addressed team@host
            // means only that key's signed receipt settles the entry.
            if let Some(key) = trusted_key(state, &to_fleet, &to_host) {
                let signed_ok = receipt["relay"]["pubkey"].as_str() == Some(key.as_str())
                    && verify_receipt(&receipt, &to_fleet, &to_host);
                if !signed_ok {
                    log_event(
                        state,
                        &json!({"event": "relay-in", "outcome": "receipt-refused", "route": route, "peer_sender": peer_sender, "id": id, "detail": "receipt signature missing or invalid for the confirmed key"}),
                    );
                    return Ok(result(
                        "receipt-refused",
                        "receipt is not signed by the addressed team's confirmed key",
                    ));
                }
                let current = entry["state"].as_str().unwrap_or("").to_string();
                if crate::sender::is_terminal(&current) && current != receipt_state {
                    return Ok(result(
                        "receipt-ignored-terminal",
                        format!("outbox already terminal ({current})"),
                    ));
                }
                entry["state"] = json!(crate::sender::map_receiver_state(&receipt_state));
                let _ = write_json_atomic(&crate::sender::outbox_path(state, &id), &entry);
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "receipt-recorded", "route": route, "id": id, "state": receipt_state}),
                );
                return Ok(result(
                    "receipt-recorded",
                    format!("signed receipt {receipt_state} for {id}"),
                ));
            }
            let poster_ok =
                confirmed_mapping_for(state, peer_sender, route).is_some_and(|(fleet, host)| {
                    fleet == to_fleet && host.as_deref() == Some(to_host.as_str())
                }) || load_declarations(state).is_ok_and(|declarations| {
                    declarations.iter().any(|(_, _, declaration)| {
                        declaration.relay_channels.iter().any(|entry| {
                            (entry.route == route
                                && (entry.channel.is_empty() || entry.channel == channel))
                                && entry.peers.get(peer_sender).is_some_and(|mapped| {
                                    let (fleet, host) = match mapped.split_once('@') {
                                        Some((fleet, host)) => (fleet, Some(host)),
                                        None => (mapped.as_str(), None),
                                    };
                                    fleet == to_fleet && host == Some(to_host.as_str())
                                })
                        })
                    })
                });
            if !poster_ok {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "receipt-refused", "route": route, "peer_sender": peer_sender, "id": id, "detail": "poster not mapped to the addressed fleet"}),
                );
                return Ok(result(
                    "receipt-refused",
                    "receipt poster is not mapped to the addressed fleet",
                ));
            }
            let current = entry["state"].as_str().unwrap_or("").to_string();
            if crate::sender::is_terminal(&current) && current != receipt_state {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "receipt-ignored-terminal", "route": route, "id": id, "state": receipt_state}),
                );
                return Ok(result(
                    "receipt-ignored-terminal",
                    format!("outbox already terminal ({current})"),
                ));
            }
            let mapped = crate::sender::map_receiver_state(&receipt_state);
            entry["state"] = json!(mapped);
            if mapped == "deferred" {
                // Kept beside the state, as the sender walk does, so `outbox`
                // shows why; `pump` retries it once the target is fixed.
                entry["undeliverable"] = receipt
                    .get("detail")
                    .cloned()
                    .unwrap_or_else(|| json!("undeliverable"));
            } else if let Some(entry) = entry.as_object_mut() {
                entry.remove("undeliverable");
            }
            let _ = write_json_atomic(&crate::sender::outbox_path(state, &id), &entry);
            log_event(
                state,
                &json!({"event": "relay-in", "outcome": "receipt-recorded", "route": route, "id": id, "state": receipt_state}),
            );
            Ok(result(
                "receipt-recorded",
                format!("receipt {receipt_state} for {id}"),
            ))
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
        let mut receipt = json!({"schema": "agent-teams-receipt/v1", "id": id, "state": state_name, "detail": detail, "at": format_rfc3339_ms(now_unix_ms())});
        // Receipts are signed with this (receiving) team@host's key.
        let _ = sign_receipt(state, &mut receipt, &declaration.team, this_host);
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
    // Signed identity (the default): authority comes from the envelope's
    // signature by the sender team@host's confirmed key; the poster ID
    // only gates "a listed relay bot". Our own signed posts are ignored.
    let embedded_key = envelope["relay"]["pubkey"].as_str();
    // Our own post loops back only when the sender is the receiving
    // team itself on this host: another local team's signed post is a
    // genuine peer delivery, not a loop.
    if from_team == declaration.team && from_host == this_host {
        if let Some(own) = own_public_key(state, &from_team, &from_host) {
            if embedded_key == Some(own.as_str()) && verify_envelope(envelope) {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "ignored-own-post", "route": route, "id": id}),
                );
                return Ok(result("ignored-own-post", "signed by our own key"));
            }
        }
    }
    // Another local team's signed copy (#806 relay-in-ignore-own-host): a message between two
    // teams on this host is delivered directly and also mirrored into the relay channel, so
    // this host reads its own copy back. Its authority is the signing key that team@host keeps
    // on this host (relay/keys), not a trusted peer key: verified by it, the copy takes the
    // ledger path like any admitted envelope (`duplicate-known` after the direct delivery,
    // delivered when the relay copy is the only one), never a stranger. A signed copy claiming
    // this host that no local key verifies is an impersonation: quarantined with an alarm.
    if from_host == this_host && embedded_key.is_some() {
        let local = own_public_key(state, &from_team, &from_host);
        if local.is_some() && embedded_key == local.as_deref() && verify_envelope(envelope) {
            log_event(
                state,
                &json!({"event": "relay-in", "outcome": "own-host", "route": route, "id": id, "from_team": from_team}),
            );
            return relay_deliver(
                state,
                this_host,
                route,
                peer_sender,
                channel,
                thread,
                text,
                envelope,
                declaration,
                entry,
                &id,
                true,
            );
        }
        let reason = if local.is_none() {
            "claims this host, but this host keeps no signing key for the sender team"
        } else if embedded_key != local.as_deref() {
            "claims this host, but is not signed with this host's key for the sender team"
        } else {
            "claims this host, but its signature does not verify"
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
            "stranger: claims this host without its signing key; alarm raised to the coordinator",
        );
        out.outcome = "stranger-quarantined".to_string();
        return Ok(post_receipt(state, entry, channel, thread, &id, out));
    }
    let trusted = trusted_key(state, &from_team, &from_host);
    let signed_mode = trusted.is_some() || embedded_key.is_some();
    if signed_mode {
        let admitted_signed = trusted.is_some()
            && embedded_key == trusted.as_deref()
            && entry.peers.contains_key(peer_sender)
            && verify_envelope(envelope);
        if !admitted_signed {
            let reason = if trusted.is_none() {
                "unknown signing key: no confirmed key for the sender team"
            } else if embedded_key != trusted.as_deref() {
                "signing key does not match the confirmed key for the sender team"
            } else if !entry.peers.contains_key(peer_sender) {
                "poster is not a listed relay bot"
            } else {
                "envelope signature does not verify"
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
                "stranger: signed identity not confirmed; alarm raised to the coordinator",
            );
            out.outcome = "stranger-quarantined".to_string();
            return Ok(post_receipt(state, entry, channel, thread, &id, out));
        }
        // Signed and admitted: fall through to the malformed checks and
        // the ledger, skipping the weaker sender-mapping admission below.
        return relay_deliver(
            state,
            this_host,
            route,
            peer_sender,
            channel,
            thread,
            text,
            envelope,
            declaration,
            entry,
            &id,
            true,
        );
    }
    // Legacy one-bot-per-team mode (weaker, documented): the declaration's
    // peer mapping, or a confirmed discovery mapping, must name this poster
    // for the envelope's sender team. Anything else addressed to this team
    // is a STRANGER event: loud, quarantined, never delivered.
    let mapped = entry
        .peers
        .get(peer_sender)
        .map(String::as_str)
        .or_else(|| {
            confirmed_mapping_for(state, peer_sender, route)
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
    relay_deliver(
        state,
        this_host,
        route,
        peer_sender,
        channel,
        thread,
        text,
        envelope,
        declaration,
        entry,
        &id,
        false,
    )
}

/// Deliver one admitted envelope: malformed checks, then the ledger.
#[allow(clippy::too_many_arguments)]
fn relay_deliver(
    state: &Path,
    this_host: &str,
    route: &str,
    peer_sender: &str,
    channel: &str,
    thread: &str,
    text: &str,
    envelope: &Value,
    declaration: &Declaration,
    entry: &crate::model::RelayChannel,
    id: &str,
    signed_verified: bool,
) -> DataResult<RelayInResult> {
    // Slack link markup rescue (#794b): if the body hash fails, undo
    // Slack's auto-link rewriting inside the body and accept only if
    // the hash (and thus the signature over it) then verifies.
    let restored: Value;
    let envelope = if crate::envelope::check_envelope(envelope)
        == Err("body-hash-mismatch".to_string())
    {
        let body = envelope.get("body").and_then(Value::as_str).unwrap_or("");
        let fixed = undo_slack_link_markup(body);
        if fixed != body {
            let mut candidate = envelope.clone();
            candidate["body"] = json!(fixed);
            if crate::envelope::check_envelope(&candidate).is_ok()
                && (envelope.get("relay").is_none() || verify_envelope(&candidate))
            {
                log_event(
                    state,
                    &json!({"event": "relay-in", "outcome": "link-markup-restored", "route": route, "peer_sender": peer_sender, "id": id}),
                );
                restored = candidate;
                &restored
            } else {
                envelope
            }
        } else {
            envelope
        }
    } else {
        envelope
    };
    let from_team = envelope
        .get("from")
        .and_then(|f| f.get("team"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let receipt_for = |state_name: &str, detail: &str| -> RelayInResult {
        let mut receipt = json!({"schema": "agent-teams-receipt/v1", "id": id, "state": state_name, "detail": detail, "at": format_rfc3339_ms(now_unix_ms())});
        let _ = sign_receipt(state, &mut receipt, &declaration.team, this_host);
        let mut out = result(state_name, detail.to_string());
        out.receipt_text = Some(render_receipt(envelope, &receipt));
        out
    };
    if !tag_matches(text, envelope) {
        // The signed bypass applies only when the tag line is absent
        // (the fence-only piece of a split post): a present-but-wrong tag
        // means the visible header disagrees with the envelope, which is
        // filed as unparseable, never silently delivered.
        let tag_absent = !text
            .lines()
            .next()
            .is_some_and(|line| line.trim_start().starts_with('['));
        if signed_verified && tag_absent {
            // A signed, verified envelope is the authority: a missing tag
            // line must not become a stranger. Deliver as usual (#763).
            log_event(
                state,
                &json!({"event": "relay-in", "outcome": "tag-missing-signed", "route": route, "peer_sender": peer_sender, "id": id}),
            );
        } else {
            // Legacy admission has no signature to trust, so a missing tag
            // is filed under the unparseable kind, never as a stranger,
            // and never replied to.
            let detail = "malformed: tag does not match envelope".to_string();
            let qid =
                record_unparseable_confirmed(state, route, peer_sender, channel, text, &detail)?;
            return Ok(result(
                NOT_RECORDED,
                format!(
                    "{detail}; quarantined as {qid} — dismiss with: \
                     agent-teams relay quarantine dismiss {qid}"
                ),
            ));
        }
    }
    if let Err(reason) = check_envelope(envelope) {
        if reason == "body-hash-mismatch" {
            // From an admitted sender this is its own integrity kind:
            // the body differs from what was hashed/signed, so it can
            // never deliver and the post is evidence (#794 review).
            let signed = envelope.get("relay").is_some();
            let qid = record_integrity_confirmed(
                state,
                route,
                peer_sender,
                channel,
                text,
                envelope,
                id,
                signed,
            )?;
            return Ok(result(
                NOT_RECORDED,
                format!(
                    "body does not match its hash; quarantined as {qid} — dismiss with: \
                     agent-teams relay quarantine dismiss {qid}"
                ),
            ));
        }
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
        return Ok(post_receipt(state, entry, channel, thread, id, out));
    }
    let team_dir = state.join("teams.d").join(&declaration.team);
    let ledger_path = crate::ledger::entry_path(state, &declaration.team, id);
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
        via_chat_relay: true,
    };
    let answer = accept(&ctx, envelope, deliverer.as_ref())?;
    let state_name = answer
        .state
        .clone()
        .unwrap_or_else(|| answer.outcome.clone());
    // A duplicate copy — a mirror post (#782) or a replay of a message
    // already delivered directly — is absorbed quietly: the receipt
    // keeps the recorded state (`delivered`, `accepted`, ...) and says
    // `<state> already`, so the owner watching the thread sees one
    // delivery, never a second one and never a stranger alarm.
    let detail = if answer.outcome == "known" {
        log_event(
            state,
            &json!({"event": "relay-in", "outcome": "duplicate-known", "route": route, "id": id, "state": state_name}),
        );
        format!("{state_name} already; duplicate copy not delivered again")
    } else {
        answer.detail.clone()
    };
    let mut out = receipt_for(&state_name, &detail);
    out.outcome = answer.outcome.clone();
    Ok(post_receipt(state, entry, channel, thread, id, out))
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

/// The confirmed mapping for one sender on one relay route, as
/// `(fleet, host)`. Confirmations learned on a different route do not
/// admit here; a confirmation without a recorded route admits anywhere.
pub fn confirmed_mapping_for(
    state: &Path,
    sender_id: &str,
    route: &str,
) -> Option<(String, Option<String>)> {
    let store = load_store(state);
    let entry = store_list(&store, "confirmed").iter().find(|c| {
        c.get("sender_id").and_then(Value::as_str) == Some(sender_id)
            && c.get("route")
                .and_then(Value::as_str)
                .is_none_or(|r| r == route)
    })?;
    let team = entry
        .get("team")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let host = entry
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some(match team.split_once('@') {
        Some((fleet, team_host)) => (fleet.to_string(), Some(team_host.to_string())),
        None if !host.is_empty() => (team, Some(host)),
        None => (team, None),
    })
}

/// Whether a sender counts as confirmed for unparseable-post handling:
/// a confirmed discovery mapping, or a peer listed in a local
/// declaration's `relay_channels[].peers` for this route/channel.
fn is_confirmed_sender(state: &Path, peer_sender: &str, route: &str, channel: &str) -> bool {
    if confirmed_mapping_for(state, peer_sender, route).is_some() {
        return true;
    }
    load_declarations(state).is_ok_and(|declarations| {
        declarations.iter().any(|(_, _, declaration)| {
            declaration.relay_channels.iter().any(|entry| {
                entry.route == route
                    && (entry.channel.is_empty() || entry.channel == channel)
                    && entry.peers.contains_key(peer_sender)
            })
        })
    })
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
    let _lock = lock_store(state)?;
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

/// Record an unparseable post from a CONFIRMED sender: quarantine it and
/// open an alarm of kind `unparseable` for the receiving team — never a
/// stranger alarm, since the sender is confirmed and the usual cause is
/// a relay post the provider split or truncated. Returns the quarantine
/// ID, which `relay quarantine dismiss` removes.
fn record_unparseable_confirmed(
    state: &Path,
    route: &str,
    peer_sender: &str,
    channel: &str,
    text: &str,
    reason: &str,
) -> DataResult<String> {
    let id = format!(
        "unparseable-{}",
        &sha256_hex(strip_provider_notes(text).as_bytes())[..12]
    );
    // A bridge rescan can re-offer the same Slack post indefinitely: a
    // given post is recorded at most once, so it cannot generate an
    // unbounded stream of events for one message (#763 item 6).
    if quarantined_posts(state).iter().any(|post| {
        post.get("id").and_then(Value::as_str) == Some(id.as_str())
            && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender)
    }) {
        return Ok(id);
    }
    let envelope = json!({"id": id, "from": Value::Null, "unparseable": true});
    let declarations = load_declarations(state).unwrap_or_default();
    let Some((_, _, declaration)) = declarations.iter().find(|(_, _, declaration)| {
        declaration.relay_channels.iter().any(|entry| {
            entry.route == route && (entry.channel.is_empty() || entry.channel == channel)
        })
    }) else {
        // No local team receives this route: quarantine without an alarm.
        let _lock = lock_store(state)?;
        let mut quarantine = load_json_list(&quarantine_path(state), "posts");
        let posts = quarantine
            .get_mut("posts")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-quarantine", "posts is not a list"))?;
        if !posts.iter().any(|post| {
            post.get("id").and_then(Value::as_str) == Some(id.as_str())
                && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender)
        }) {
            posts.push(json!({
                "id": id, "peer_sender": peer_sender, "route": route,
                "channel": channel, "envelope": envelope, "post": text,
                "reason": reason, "quarantined_at": format_rfc3339_ms(now_unix_ms()),
            }));
        }
        write_json_atomic(&quarantine_path(state), &quarantine)?;
        return Ok(id);
    };
    let entry = declaration
        .relay_channels
        .iter()
        .find(|entry| entry.route == route);
    let _lock = lock_store(state)?;
    {
        let mut quarantine = load_json_list(&quarantine_path(state), "posts");
        let posts = quarantine
            .get_mut("posts")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-quarantine", "posts is not a list"))?;
        if !posts.iter().any(|post| {
            post.get("id").and_then(Value::as_str) == Some(id.as_str())
                && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender)
        }) {
            posts.push(json!({
                "id": id, "peer_sender": peer_sender, "route": route,
                "channel": channel, "envelope": envelope, "post": text,
                "reason": reason, "quarantined_at": format_rfc3339_ms(now_unix_ms()),
            }));
            if posts.len() > 512 {
                let excess = posts.len() - 512;
                posts.drain(0..excess);
            }
        }
        write_json_atomic(&quarantine_path(state), &quarantine)?;
    }
    let now_ms = now_unix_ms();
    let mut alarm_index: Option<usize> = None;
    {
        let mut doc = load_json_list(&alarms_path(state), "alarms");
        let list = doc
            .get_mut("alarms")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-alarms", "alarms is not a list"))?;
        for (index, alarm) in list.iter().enumerate() {
            if alarm.get("sender_id").and_then(Value::as_str) == Some(peer_sender)
                && alarm.get("local_team").and_then(Value::as_str)
                    == Some(declaration.team.as_str())
                && alarm.get("kind").and_then(Value::as_str) == Some("unparseable")
                && alarm.get("state").and_then(Value::as_str) == Some("open")
            {
                alarm_index = Some(index);
                break;
            }
        }
        if let Some(index) = alarm_index {
            list[index]["reason"] = json!(reason);
            list[index]["quarantine_id"] = json!(id);
            let mut ids: Vec<String> = list[index]
                .get("quarantine_ids")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if !ids.iter().any(|existing| existing == &id) {
                ids.push(id.clone());
            }
            list[index]["quarantine_ids"] = json!(ids);
            list[index]["interval_seconds"] =
                json!(entry.map_or(1800, |entry| entry.alarm_interval_seconds));
        } else {
            list.push(json!({
                "sender_id": peer_sender, "local_team": declaration.team,
                "kind": "unparseable", "quarantine_id": id, "quarantine_ids": [id],
                "claimed_from": Value::Null, "reason": reason,
                "state": "open", "first_at": format_rfc3339_ms(now_ms),
                "last_raised_at": Value::Null, "raises": 0,
                "interval_seconds": entry.map_or(1800, |entry| entry.alarm_interval_seconds),
            }));
            alarm_index = Some(list.len() - 1);
        }
        write_json_atomic(&alarms_path(state), &doc)?;
    }
    log_event(
        state,
        &json!({"event": "unparseable", "sender_id": peer_sender, "local_team": declaration.team, "id": id, "reason": reason}),
    );
    if let Some(index) = alarm_index {
        let doc = load_json_list(&alarms_path(state), "alarms");
        let due = doc
            .get("alarms")
            .and_then(Value::as_array)
            .and_then(|list| list.get(index))
            .is_none_or(|alarm| {
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
                last_ms == 0 || now_ms - last_ms >= interval_ms
            });
        if due {
            let _ = raise_alarm_at(state, index, now_ms);
        }
    }
    Ok(id)
}

/// Record a body-hash mismatch from an admitted sender under the
/// `integrity` alarm kind (#794): the body differs from what its sender
/// hashed (and, when signed, signed), so it can never deliver and the
/// quarantined raw post is the evidence. Kept apart from the routine
/// unparseable alarm so one dismiss cannot close both. Returns the
/// quarantine ID (the envelope's message ID).
#[allow(clippy::too_many_arguments)]
fn record_integrity_confirmed(
    state: &Path,
    route: &str,
    peer_sender: &str,
    channel: &str,
    text: &str,
    envelope: &Value,
    id: &str,
    signed: bool,
) -> DataResult<String> {
    let reason = format!(
        "body does not match its {} hash (after undoing Slack link markup): \
         possible tampering or an unrecognised provider rewrite; not delivered",
        if signed { "signed" } else { "declared" }
    );
    let declarations = load_declarations(state).unwrap_or_default();
    let Some((_, _, declaration)) = declarations.iter().find(|(_, _, declaration)| {
        declaration.relay_channels.iter().any(|entry| {
            entry.route == route && (entry.channel.is_empty() || entry.channel == channel)
        })
    }) else {
        return Ok(id.to_string());
    };
    let entry = declaration
        .relay_channels
        .iter()
        .find(|entry| entry.route == route);
    let _lock = lock_store(state)?;
    {
        let mut quarantine = load_json_list(&quarantine_path(state), "posts");
        let posts = quarantine
            .get_mut("posts")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-quarantine", "posts is not a list"))?;
        if !posts.iter().any(|post| {
            post.get("id").and_then(Value::as_str) == Some(id)
                && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender)
        }) {
            posts.push(json!({
                "id": id, "peer_sender": peer_sender, "route": route,
                "channel": channel, "envelope": envelope, "post": text,
                "reason": reason, "quarantined_at": format_rfc3339_ms(now_unix_ms()),
            }));
            if posts.len() > 512 {
                let excess = posts.len() - 512;
                posts.drain(0..excess);
            }
        }
        write_json_atomic(&quarantine_path(state), &quarantine)?;
    }
    let now_ms = now_unix_ms();
    let mut alarm_index: Option<usize> = None;
    {
        let mut doc = load_json_list(&alarms_path(state), "alarms");
        let list = doc
            .get_mut("alarms")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-alarms", "alarms is not a list"))?;
        for (index, alarm) in list.iter().enumerate() {
            if alarm.get("sender_id").and_then(Value::as_str) == Some(peer_sender)
                && alarm.get("local_team").and_then(Value::as_str)
                    == Some(declaration.team.as_str())
                && alarm.get("kind").and_then(Value::as_str) == Some("integrity")
                && alarm.get("state").and_then(Value::as_str) == Some("open")
            {
                alarm_index = Some(index);
                break;
            }
        }
        if let Some(index) = alarm_index {
            list[index]["reason"] = json!(reason);
            list[index]["quarantine_id"] = json!(id);
            let mut ids: Vec<String> = list[index]
                .get("quarantine_ids")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
            list[index]["quarantine_ids"] = json!(ids);
        } else {
            list.push(json!({
                "sender_id": peer_sender, "local_team": declaration.team,
                "kind": "integrity", "quarantine_id": id, "quarantine_ids": [id],
                "claimed_from": envelope.get("from"), "reason": reason,
                "state": "open", "first_at": format_rfc3339_ms(now_ms),
                "last_raised_at": Value::Null, "raises": 0,
                "interval_seconds": entry.map_or(1800, |entry| entry.alarm_interval_seconds),
            }));
            alarm_index = Some(list.len() - 1);
        }
        write_json_atomic(&alarms_path(state), &doc)?;
    }
    log_event(
        state,
        &json!({"event": "integrity", "sender_id": peer_sender, "local_team": declaration.team, "id": id, "reason": reason}),
    );
    if let Some(index) = alarm_index {
        let _ = raise_alarm_at(state, index, now_ms);
    }
    Ok(id.to_string())
}

/// Record an unparseable post as a STRANGER event. No envelope survived
/// parsing, so quarantine is keyed by a hash of the raw post and the alarm
/// carries no claimed team or host.
fn record_unparseable_stranger(
    state: &Path,
    route: &str,
    peer_sender: &str,
    channel: &str,
    text: &str,
    reason: &str,
) -> DataResult<()> {
    let id = format!(
        "unparseable-{}",
        &sha256_hex(strip_provider_notes(text).as_bytes())[..12]
    );
    let envelope = json!({"id": id, "from": Value::Null, "unparseable": true});
    let declarations = load_declarations(state).unwrap_or_default();
    for (_, _, declaration) in &declarations {
        if let Some(entry) = declaration.relay_channels.iter().find(|entry| {
            entry.route == route && (entry.channel.is_empty() || entry.channel == channel)
        }) {
            return record_stranger(
                state,
                declaration,
                entry,
                peer_sender,
                &envelope,
                text,
                reason,
            );
        }
    }
    // No declaration matches this route: quarantine durably without
    // alarming an unrelated team (the post may belong to another fleet).
    let _lock = lock_store(state)?;
    let mut quarantine = load_json_list(&quarantine_path(state), "posts");
    let posts = quarantine
        .get_mut("posts")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-quarantine", "posts is not a list"))?;
    if !posts.iter().any(|post| {
        post.get("id").and_then(Value::as_str) == Some(id.as_str())
            && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender)
    }) {
        posts.push(json!({
            "id": id, "peer_sender": peer_sender, "route": route,
            "channel": channel, "envelope": envelope, "post": text,
            "reason": reason, "quarantined_at": format_rfc3339_ms(now_unix_ms()),
        }));
    }
    write_json_atomic(&quarantine_path(state), &quarantine)
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
    let _lock = lock_store(state)?;
    let id = envelope
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // Already quarantined for this sender: a rescan re-offering the same
    // post must not log or re-alarm again (#763 item 6).
    if quarantined_posts(state).iter().any(|post| {
        post.get("id").and_then(Value::as_str) == Some(id.as_str())
            && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender)
    }) {
        return Ok(());
    }
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
            // Bounded: a flooding stranger cannot grow this without limit.
            if posts.len() > 512 {
                let excess = posts.len() - 512;
                posts.drain(0..excess);
            }
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
    // Raise at once for a new alarm, then only when due: a flooding
    // stranger coalesces into one alarm plus timer repeats, never one
    // coordinator notice per post.
    if let Some(index) = alarm_index {
        let doc = load_json_list(&alarms_path(state), "alarms");
        let due = doc
            .get("alarms")
            .and_then(Value::as_array)
            .and_then(|list| list.get(index))
            .is_none_or(|alarm| {
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
                last_ms == 0 || now_ms - last_ms >= interval_ms
            });
        if due {
            let _ = raise_alarm_at(state, index, now_ms);
        }
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
    // Alarm fields are attacker-shaped until validated: only fleet/host
    // names that pass the same checks as configuration, and single-line
    // sender/reason text, may appear in the notice a coordinator reads.
    let clean = |value: Option<&str>| -> String {
        value
            .unwrap_or("?")
            .chars()
            .filter(|c| !c.is_control() && *c != '[' && *c != ']')
            .take(128)
            .collect::<String>()
            .trim()
            .to_string()
    };
    // Peer-controlled names never enter a coordinator notice verbatim:
    // a name that fails validation renders as <invalid>, not cleaned text.
    let claimed_team = claimed.get("team").and_then(Value::as_str).unwrap_or("");
    let claimed_host = claimed.get("host").and_then(Value::as_str).unwrap_or("");
    let claimed_team = if crate::model::valid_team_name(claimed_team) {
        claimed_team.to_string()
    } else {
        "<invalid>".to_string()
    };
    let claimed_host = if crate::model::valid_host_name(claimed_host) {
        claimed_host.to_string()
    } else {
        "<invalid>".to_string()
    };
    let sender = clean(Some(&sender));
    let rendered = if alarm.get("kind").and_then(Value::as_str) == Some("integrity") {
        format!(
            "[relay alarm: body does not match its signed hash from confirmed sender {sender}]\n\
             [team message: kind alarm; ESCALATE TO OWNER via this team's normal owner channel; reason: {}; post quarantined as evidence, not delivered]\n\
             A confirmed sender's envelope body differs from the hash it carries, after undoing Slack link markup: possible tampering or an unrecognised provider rewrite. It can never be delivered, and this alarm repeats until it is dismissed.\n\
             Quarantined: {}\n\
             Dismiss: agent-teams relay quarantine dismiss {}   Status: agent-teams relay status\n",
            clean(alarm.get("reason").and_then(Value::as_str)),
            alarm
                .get("quarantine_ids")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(|id| clean(Some(id)))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|joined| !joined.is_empty())
                .unwrap_or_else(|| clean(alarm.get("quarantine_id").and_then(Value::as_str))),
            clean(alarm.get("quarantine_id").and_then(Value::as_str)),
        )
    } else if alarm.get("kind").and_then(Value::as_str) == Some("unparseable") {
        format!(
            "[relay alarm: unparseable post from confirmed sender {sender}]\n\
             [team message: kind alarm; ESCALATE TO OWNER via this team's normal owner channel; reason: {}; post quarantined, not delivered]\n\
             The confirmed relay bot posted something agent-teams could not parse (a relay post the provider split or truncated is the usual cause). It is quarantined, not delivered, and this alarm repeats until it is dismissed.\n\
             Quarantined: {}\n\
             Dismiss: agent-teams relay quarantine dismiss {}   Status: agent-teams relay status\n",
            clean(alarm.get("reason").and_then(Value::as_str)),
            alarm
                .get("quarantine_ids")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(|id| clean(Some(id)))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|joined| !joined.is_empty())
                .unwrap_or_else(|| {
                    clean(alarm.get("quarantine_id").and_then(Value::as_str))
                }),
            clean(alarm.get("quarantine_id").and_then(Value::as_str)),
        )
    } else {
        format!(
            "[stranger alarm: unconfirmed sender {sender} in the chat relay]\n\
             [team message: kind alarm; ESCALATE TO OWNER via this team's normal owner channel; sender {sender} claims {}@{}; reason: {}; posts quarantined, not delivered]\n\
             A bot the relay does not confirm posted to team {local_team} in the relay channel. It is quarantined, not delivered, and this alarm repeats until the sender is confirmed or banned.\n\
             Confirm: agent-teams relay confirm {sender}   Ban: agent-teams relay ban {sender}   Status: agent-teams relay status\n",
            claimed_team,
            claimed_host,
            clean(alarm.get("reason").and_then(Value::as_str)),
        )
    };
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
    let raised = matches!(outcome, crate::deliver::Handover::Accepted);
    // Only a successful hand-over is stamped as raised: a failed raise
    // stays due, so the next relay activity retries it instead of
    // silently waiting out a full interval.
    if raised {
        let list = doc
            .get_mut("alarms")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| DataError::new("relay-alarms", "alarms is not a list"))?;
        if let Some(alarm) = list.get_mut(index) {
            alarm["last_raised_at"] = json!(format_rfc3339_ms(now_ms));
            alarm["raises"] = json!(alarm.get("raises").and_then(Value::as_u64).unwrap_or(0) + 1);
        }
        write_json_atomic(&alarms_path(state), &doc)?;
    }
    Ok(raised)
}

/// Raise every open alarm whose repeat is due. Called by `relay-in`,
/// `pump`, and `relay alarms`, so no daemon is needed: any relay activity
/// or pump run re-raises overdue stranger alarms.
pub fn raise_due_alarms(state: &Path, now_ms: i64) -> DataResult<Vec<String>> {
    let _lock = lock_store(state)?;
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

/// Close any open or banned alarm for a sender (after confirmation).
pub fn close_alarm(state: &Path, sender_id: &str) -> DataResult<()> {
    let _lock = lock_store(state)?;
    let mut doc = load_json_list(&alarms_path(state), "alarms");
    if let Some(list) = doc.get_mut("alarms").and_then(Value::as_array_mut) {
        for alarm in list.iter_mut() {
            if alarm.get("sender_id").and_then(Value::as_str) == Some(sender_id)
                && matches!(
                    alarm.get("state").and_then(Value::as_str),
                    Some("open") | Some("banned")
                )
            {
                alarm["state"] = json!("confirmed");
            }
        }
    }
    write_json_atomic(&alarms_path(state), &doc)
}

/// Remove a sender's ban (the owner's reversal of `relay ban`): its open
/// alarm can then be confirmed, and its quarantined posts stay held until
/// confirmation like anyone else's.
pub fn unban_sender(state: &Path, sender_id: &str) -> DataResult<()> {
    let _lock = lock_store(state)?;
    let mut doc = load_json_list(&alarms_path(state), "alarms");
    if let Some(list) = doc.get_mut("alarms").and_then(Value::as_array_mut) {
        for alarm in list.iter_mut() {
            if alarm.get("sender_id").and_then(Value::as_str) == Some(sender_id)
                && alarm.get("state").and_then(Value::as_str) == Some("banned")
            {
                alarm["state"] = json!("open");
            }
        }
    }
    write_json_atomic(&alarms_path(state), &doc)
}

/// What one quarantine drain did: posts delivered (or already known at the
/// ledger), and posts dropped with their terminal ledger state named.
pub struct DrainOutcome {
    /// IDs accepted or already known at the receiving ledger.
    pub delivered: Vec<String>,
    /// `(id, state)` posts the ledger terminally refused (expired, ...).
    pub dropped: Vec<(String, String)>,
}

/// Deliver quarantined posts whose sender is now confirmed, in arrival
/// order, exactly once (the ledger claim is the dedupe). Returns the IDs
/// delivered (or already known). Malformed posts stay quarantined.
pub fn drain_quarantine(state: &Path, this_host: &str) -> DataResult<DrainOutcome> {
    let _lock = lock_store(state)?;
    let posts = quarantined_posts(state);
    let mut delivered: Vec<String> = Vec::new();
    let mut dropped: Vec<(String, String)> = Vec::new();
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
                .is_none_or(|text| !tag_matches(text, &envelope))
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
            let admitted = declaration.relay_channels.iter().any(|entry| {
                entry.peers.get(&peer_sender).is_some_and(|mapped| {
                    let (fleet, host) = match mapped.split_once('@') {
                        Some((fleet, host)) => (fleet, Some(host)),
                        None => (mapped.as_str(), None),
                    };
                    fleet == from_team && host.is_none_or(|host| host == from_host)
                })
            }) || confirmed_mapping_for(
                state,
                &peer_sender,
                post.get("route").and_then(Value::as_str).unwrap_or(""),
            )
            .is_some_and(|(fleet, host)| {
                fleet.as_str() == from_team && host.as_deref().is_none_or(|host| host == from_host)
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
                via_chat_relay: true,
            };
            match accept(&ctx, &envelope, deliverer.as_ref()) {
                // Only accepted/known count as delivered. A terminal
                // refusal is reported as dropped with its state, logged,
                // and removed — never silently counted as delivered.
                // Undeliverable (#721): nothing was claimed and the target
                // may be fixed, so the post stays quarantined for the next
                // drain instead of being dropped.
                Ok(answer) if answer.outcome == "undeliverable" => {
                    log_event(
                        state,
                        &json!({"event": "relay-quarantine", "outcome": "undeliverable-kept", "id": id, "peer_sender": peer_sender}),
                    );
                    false
                }
                Ok(answer) if matches!(answer.outcome.as_str(), "accepted" | "known") => {
                    delivered.push(id);
                    log_event(
                        state,
                        &json!({"event": "relay-quarantine", "outcome": answer.outcome, "id": delivered.last(), "peer_sender": peer_sender}),
                    );
                    true
                }
                Ok(answer) => {
                    dropped.push((id.clone(), answer.state.unwrap_or(answer.outcome)));
                    log_event(
                        state,
                        &json!({"event": "relay-quarantine", "outcome": "dropped", "id": id, "peer_sender": peer_sender}),
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
    Ok(DrainOutcome { delivered, dropped })
}

/// Remove exactly one quarantined post (by quarantine ID) if present.
fn remove_quarantined(state: &Path, id: &str, peer_sender: &str) -> DataResult<bool> {
    let _lock = lock_store(state)?;
    let mut quarantine = load_json_list(&quarantine_path(state), "posts");
    let posts = quarantine
        .get_mut("posts")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DataError::new("relay-quarantine", "posts is not a list"))?;
    let before = posts.len();
    posts.retain(|post| {
        !(post.get("id").and_then(Value::as_str) == Some(id)
            && post.get("peer_sender").and_then(Value::as_str) == Some(peer_sender))
    });
    let removed = posts.len() != before;
    if removed {
        write_json_atomic(&quarantine_path(state), &quarantine)?;
    }
    Ok(removed)
}

/// Reprocess quarantined posts through `relay_in` with the current
/// parser and trust state. This is how posts quarantined by a since-
/// fixed parser (or before their sender was confirmed) are recovered:
/// each stored raw post is replayed exactly as the bridge would deliver
/// it now. A post whose new outcome is anything other than
/// stranger-quarantined / not-recorded is consumed and removed from
/// quarantine; one that still fails stays quarantined and loud.
/// With `only_id`, only that quarantine ID is retried. Returns
/// `(quarantine id, outcome)` for every post tried.
pub fn retry_quarantine(
    state: &Path,
    this_host: &str,
    only_id: Option<&str>,
) -> DataResult<Vec<(String, String)>> {
    let mut results = Vec::new();
    for post in quarantined_posts(state) {
        let id = post
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if only_id.is_some_and(|only| only != id) {
            continue;
        }
        let peer_sender = post
            .get("peer_sender")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(raw) = post.get("post").and_then(Value::as_str) else {
            results.push((id, "no-stored-post".to_string()));
            continue;
        };
        let route = post.get("route").and_then(Value::as_str).unwrap_or("");
        let channel = post.get("channel").and_then(Value::as_str).unwrap_or("");
        let out = relay_in(
            state,
            this_host,
            route,
            &peer_sender,
            channel,
            "",
            false,
            raw,
        )?;
        let consumed = !matches!(out.outcome.as_str(), "stranger-quarantined" | NOT_RECORDED);
        if consumed {
            remove_quarantined(state, &id, &peer_sender)?;
        }
        log_event(
            state,
            &json!({"event": "relay-quarantine", "outcome": format!("retry:{}", out.outcome), "id": id, "peer_sender": peer_sender}),
        );
        results.push((id, out.outcome));
    }
    Ok(results)
}

/// Drop one quarantined post by quarantine ID, without delivering it:
/// the owner's explicit discard for a post that will never be admitted.
/// Returns whether an entry was removed.
pub fn drop_quarantined(state: &Path, id: &str) -> DataResult<bool> {
    let peer_sender = quarantined_posts(state)
        .iter()
        .find(|post| post.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|post| {
            post.get("peer_sender")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let Some(peer_sender) = peer_sender else {
        return Ok(false);
    };
    let removed = remove_quarantined(state, id, &peer_sender)?;
    if removed {
        // Dismissing closes the unparseable alarm that named this post:
        // otherwise raise_due_alarms re-raises it every interval forever.
        let _lock = lock_store(state)?;
        let mut doc = load_json_list(&alarms_path(state), "alarms");
        if let Some(list) = doc.get_mut("alarms").and_then(Value::as_array_mut) {
            for alarm in list.iter_mut() {
                if !matches!(
                    alarm.get("kind").and_then(Value::as_str),
                    Some("unparseable") | Some("integrity")
                ) {
                    continue;
                }
                let names_id = alarm.get("quarantine_id").and_then(Value::as_str) == Some(id)
                    || alarm
                        .get("quarantine_ids")
                        .and_then(Value::as_array)
                        .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(id)));
                if names_id && alarm.get("state").and_then(Value::as_str) == Some("open") {
                    // Remove only this post from the alarm; it closes
                    // only when no quarantined post remains under it.
                    let remaining: Vec<String> = alarm
                        .get("quarantine_ids")
                        .and_then(Value::as_array)
                        .map(|ids| {
                            ids.iter()
                                .filter_map(Value::as_str)
                                .filter(|existing| *existing != id)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    if remaining.is_empty() {
                        alarm["quarantine_ids"] = json!([]);
                        alarm["state"] = json!("closed");
                        alarm["closed_at"] = json!(format_rfc3339_ms(now_unix_ms()));
                    } else {
                        alarm["quarantine_id"] = json!(remaining.last());
                        alarm["quarantine_ids"] = json!(remaining);
                    }
                }
            }
        }
        write_json_atomic(&alarms_path(state), &doc)?;
        log_event(
            state,
            &json!({"event": "relay-quarantine", "outcome": "dropped-by-owner", "id": id, "peer_sender": peer_sender}),
        );
    }
    Ok(removed)
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
    fn long_body_truncates_the_visible_copy_never_the_envelope() {
        // #763: a long body must not produce a post the outbound helper
        // would split (its first chunk would have no fence).
        let envelope = crate::envelope::build_envelope(
            "tm-1",
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
            &"x".repeat(2400),
        );
        let post = render_post(&envelope);
        assert!(
            post.len() <= DEFAULT_MAX_POST_BYTES,
            "post is {} bytes",
            post.len()
        );
        assert!(post.contains("relay copy truncated"));
        // Visible copy is a ~500-char preview: the post is ~1x the body.
        let visible = post.split("```").next().unwrap_or("");
        assert!(
            visible.len() < 700,
            "visible preview: {} bytes",
            visible.len()
        );
        assert_eq!(
            parse_post(&post).expect("parse"),
            RelayPost::Envelope(envelope),
            "the envelope survives whole"
        );
    }

    #[test]
    fn post_size_is_measured_after_escaping() {
        // #763 review: the live converted-mode split limit is 4_000 bytes
        // after mrkdwn escaping, so `&` counts as 5 bytes, not 1.
        let envelope = crate::envelope::build_envelope(
            "tm-esc",
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
            &"&".repeat(400),
        );
        let post = render_post(&envelope);
        let fence_at = post.find("```").expect("fence");
        let (visible, fence) = post.split_at(fence_at);
        let measured = escaped_len(visible.trim_end_matches('\n')) + 1 + fence.len();
        assert!(
            measured <= DEFAULT_MAX_POST_BYTES,
            "escaped post is {measured} bytes"
        );
        assert!(post.contains("truncated"));
        assert_eq!(
            parse_post(&post).expect("parse"),
            RelayPost::Envelope(envelope),
            "the envelope survives whole"
        );
    }

    #[test]
    fn fence_escapes_slack_linked_chars_and_undo_restores_bodies() {
        // #794: a body naming files, a URL and an email round-trips.
        let envelope = crate::envelope::build_envelope(
            "tm-link",
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
            "touch cli.rs, see https://example.com/a.b and mail a@b.co & <x>",
        );
        let post = render_post(&envelope);
        let fence = post.split("```agent-teams-envelope").nth(1).expect("fence");
        assert!(!fence.contains("cli.rs"), "escaped fence: {fence}");
        assert!(!fence.contains("https://"), "escaped fence: {fence}");
        assert_eq!(
            parse_post(&post).expect("parse"),
            RelayPost::Envelope(envelope),
            "escaped fence parses to the identical envelope"
        );
        // Receiver undo of Slack's markup restores the original body.
        assert_eq!(
            undo_slack_link_markup(
                "touch <http://cli.rs|cli.rs>, see <https://example.com/a.b|https://example.com/a.b> and mail <mailto:a@b.co|a@b.co>"
            ),
            "touch cli.rs, see https://example.com/a.b and mail a@b.co"
        );
        assert_eq!(
            undo_slack_link_markup("bare <https://example.com/x> stays"),
            "bare https://example.com/x stays"
        );
        assert_eq!(
            undo_slack_link_markup("keep <https://other|label> apart"),
            "keep <https://other|label> apart",
            "non-matching markup is not rewritten"
        );
    }

    #[test]
    fn preview_cut_inside_a_fence_leaves_no_unclosed_fence() {
        // #763 review fix 5/4: a preview cut inside a code block must not
        // leave an unclosed fence swallowing the envelope, and Markdown
        // structure is stripped so conversion cannot grow the preview.
        let body = format!(
            "# Heading\n- bullet with [label](https://example.invalid/very/long/url)\n```\n{}\n```\n",
            "code line\n".repeat(200)
        );
        let envelope = crate::envelope::build_envelope(
            "tm-fence",
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
            &body,
        );
        let post = render_post(&envelope);
        let visible = post.split("```agent-teams-envelope").next().unwrap_or("");
        assert!(!visible.contains("```"), "no fence in preview: {visible}");
        assert!(
            !visible.contains("example.invalid"),
            "link URL stripped: {visible}"
        );
        assert!(
            !visible.contains("# Heading"),
            "heading stripped: {visible}"
        );
        assert_eq!(
            parse_post(&post).expect("parse"),
            RelayPost::Envelope(envelope)
        );
    }

    #[test]
    fn slack_mangled_shapes_parse_by_content() {
        // Bare fence, no info string (Slack strips it): announce.
        let announce = build_announce("sim", "host-b", "UBOTSIM", "x");
        let post = render_announce(&announce).replace("```agent-teams-announce", "```");
        assert_eq!(
            parse_post(&post).expect("bare-fence announce"),
            RelayPost::Announce(announce)
        );
        // Envelope with the tag stripped and HTML entities escaped.
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
        let mangled = render_post(&envelope)
            .replace("```agent-teams-envelope", "```")
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        assert_eq!(
            parse_post(&mangled).expect("mangled envelope"),
            RelayPost::Envelope(envelope.clone())
        );
        // Receipt with no tag at all (bare JSON) and smart quotes in detail.
        let receipt = json!({"schema": "agent-teams-receipt/v1", "id": "tm-sim-0123456789abcdef", "state": "accepted", "detail": "ok", "at": "2026-01-01T00:00:00Z"});
        let bare = serde_json::to_string(&receipt).unwrap();
        assert_eq!(
            parse_post(&bare).expect("bare receipt"),
            RelayPost::Receipt(receipt)
        );
        // Pre-schema receipt in a bare fence still classifies as a receipt.
        let legacy = "```\n{\"id\":\"tm-x\",\"state\":\"accepted\"}\n```";
        assert!(matches!(
            parse_post(legacy).expect("legacy receipt"),
            RelayPost::Receipt(_)
        ));
    }

    #[test]
    fn repo_ids_match_across_url_spellings() {
        let a = repo_id("https://github.com/Example/Repo.git");
        assert_eq!(a, repo_id("git@github.com:example/repo"));
        assert_eq!(a, repo_id("https://github.com/example/repo/"));
        assert_ne!(a, repo_id("https://github.com/example/other"));
    }

    #[test]
    fn receipt_header_names_the_receipts_real_direction() {
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
            "hello",
        );
        let receipt = json!({"schema": "agent-teams-receipt/v1", "id": "tm-sim-0123456789abcdef", "state": "accepted", "detail": "ok", "at": "2026-01-01T00:00:00Z"});
        let text = render_receipt(&envelope, &receipt);
        // From the receiving side (tools@host-a), to the original sender
        // (sim@host-b), same ID, then `receipt: accepted`.
        assert!(
            text.starts_with(
                "[from tools@host-a via agent-teams · to: sim@host-b · id: tm-sim-0123456789abcdef] receipt: accepted\n"
            ),
            "{text}"
        );
        // The inbound parser still accepts it as this message's receipt.
        assert_eq!(
            parse_post(&text).expect("parse receipt post"),
            RelayPost::Receipt(receipt)
        );
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
