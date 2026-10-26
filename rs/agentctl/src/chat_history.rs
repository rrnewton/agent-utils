//! Rust edition of `agentctl chat history` (#545 rust-primary-agentctl-chat-readers).
//!
//! Reads every retained reply body for one request from the durable bridge
//! state the polling bridge writes (`reply_storage` 2 artifacts under
//! `replies/items/`). This is a pure reader: it takes the same shared lock on
//! `.bridge.lock`, validates the request summary and every reply artifact the
//! way the existing reader does, and prints the same JSON document. The other
//! edition remains the writer until the launch slice moves; this reader must
//! therefore treat its on-disk format as a foreign, versioned format — the
//! reply item identity is a UUIDv5 derivation, implemented here (with the
//! SHA-1 it needs) rather than hand-maintained a second time in the other edition's terms.

use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde_json::Value;

const MAX_REPLY_ITEMS: i64 = 1_000_000;
const MAX_REPLY_BYTES: i64 = 30_000;
const REPLY_BUCKET_SIZE: i64 = 1_000;
const ALLOWED_ITEM_FIELDS: [&str; 8] = [
    "reply_key",
    "request_id",
    "reply_source",
    "text",
    "reply_ordinal",
    "reply_shift_from",
    "reply_id",
    "replied_at",
];

type Result<T> = std::result::Result<T, String>;

fn invalid(message: impl Into<String>) -> String {
    message.into()
}

/// One JSON value that remembers object field order, for byte-faithful output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ordered {
    Null,
    Bool(bool),
    Int(i64),
    /// A non-integer number, kept as written; validation rejects these in items.
    RawNumber(String),
    Text(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl Ordered {
    /// Build an object from ordered fields.
    pub(crate) fn object(fields: Vec<(String, Ordered)>) -> Self {
        Ordered::Object(fields)
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Ordered> {
        match self {
            Ordered::Object(fields) => fields.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn as_text(&self) -> Option<&str> {
        match self {
            Ordered::Text(text) => Some(text),
            _ => None,
        }
    }

    pub(crate) fn as_int(&self) -> Option<i64> {
        match self {
            Ordered::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// Serialize like `json.dumps(..., indent=2)` in the other edition: two-space indent and
    /// `\uXXXX` escapes for every non-ASCII character.
    pub(crate) fn dump(&self, indent: usize, out: &mut String) {
        match self {
            Ordered::Null => out.push_str("null"),
            Ordered::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Ordered::Int(value) => out.push_str(&value.to_string()),
            Ordered::RawNumber(raw) => out.push_str(raw),
            Ordered::Text(text) => dump_string(text, out),
            Ordered::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    out.push_str(&" ".repeat(indent + 2));
                    item.dump(indent + 2, out);
                    if index + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&" ".repeat(indent));
                out.push(']');
            }
            Ordered::Object(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (index, (key, value)) in fields.iter().enumerate() {
                    out.push_str(&" ".repeat(indent + 2));
                    dump_string(key, out);
                    out.push_str(": ");
                    value.dump(indent + 2, out);
                    if index + 1 < fields.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&" ".repeat(indent));
                out.push('}');
            }
        }
    }
}

fn dump_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character if (character as u32) < 0x7f => out.push(character),
            character => {
                let mut buffer = [0_u16; 2];
                for unit in character.encode_utf16(&mut buffer) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// Minimal JSON parser producing [`Ordered`]; item artifacts are small.
pub(crate) struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn parse(text: &'a str) -> Result<Ordered> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            index: 0,
        };
        let value = parser.value()?;
        parser.whitespace();
        if parser.index != parser.bytes.len() {
            return Err(invalid("trailing data after JSON document"));
        }
        Ok(value)
    }

    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.index)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.index += 1;
        }
    }

    fn peek(&mut self) -> Result<u8> {
        self.whitespace();
        self.bytes
            .get(self.index)
            .copied()
            .ok_or_else(|| invalid("unexpected end of JSON"))
    }

    fn value(&mut self) -> Result<Ordered> {
        match self.peek()? {
            b'{' => {
                self.index += 1;
                let mut fields = Vec::new();
                if self.peek()? == b'}' {
                    self.index += 1;
                    return Ok(Ordered::Object(fields));
                }
                loop {
                    let key = match self.value()? {
                        Ordered::Text(text) => text,
                        _ => return Err(invalid("object key is not a string")),
                    };
                    if self.peek()? != b':' {
                        return Err(invalid("expected ':' in object"));
                    }
                    self.index += 1;
                    let value = self.value()?;
                    fields.push((key, value));
                    match self.peek()? {
                        b',' => self.index += 1,
                        b'}' => {
                            self.index += 1;
                            return Ok(Ordered::Object(fields));
                        }
                        _ => return Err(invalid("expected ',' or '}' in object")),
                    }
                }
            }
            b'[' => {
                self.index += 1;
                let mut items = Vec::new();
                if self.peek()? == b']' {
                    self.index += 1;
                    return Ok(Ordered::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    match self.peek()? {
                        b',' => self.index += 1,
                        b']' => {
                            self.index += 1;
                            return Ok(Ordered::Array(items));
                        }
                        _ => return Err(invalid("expected ',' or ']' in array")),
                    }
                }
            }
            b'"' => Ok(Ordered::Text(self.string()?)),
            b't' => self.literal("true", Ordered::Bool(true)),
            b'f' => self.literal("false", Ordered::Bool(false)),
            b'n' => self.literal("null", Ordered::Null),
            _ => self.number(),
        }
    }

    fn literal(&mut self, word: &str, value: Ordered) -> Result<Ordered> {
        if self.bytes[self.index..].starts_with(word.as_bytes()) {
            self.index += word.len();
            Ok(value)
        } else {
            Err(invalid("invalid JSON literal"))
        }
    }

    fn number(&mut self) -> Result<Ordered> {
        let start = self.index;
        while self
            .bytes
            .get(self.index)
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E'))
        {
            self.index += 1;
        }
        if start == self.index {
            return Err(invalid("invalid JSON value"));
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.index])
            .map_err(|_| invalid("invalid number"))?;
        if !raw.contains(['.', 'e', 'E']) {
            if let Ok(value) = raw.parse::<i64>() {
                return Ok(Ordered::Int(value));
            }
        }
        Ok(Ordered::RawNumber(raw.to_owned()))
    }

    fn string(&mut self) -> Result<String> {
        if self.peek()? != b'"' {
            return Err(invalid("expected string"));
        }
        self.index += 1;
        let mut out = String::new();
        loop {
            let byte = *self
                .bytes
                .get(self.index)
                .ok_or_else(|| invalid("unterminated string"))?;
            self.index += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escape = *self
                        .bytes
                        .get(self.index)
                        .ok_or_else(|| invalid("unterminated escape"))?;
                    self.index += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            if (0xd800..0xdc00).contains(&first) {
                                if self.bytes.get(self.index) == Some(&b'\\')
                                    && self.bytes.get(self.index + 1) == Some(&b'u')
                                {
                                    self.index += 2;
                                    let second = self.hex4()?;
                                    let combined = 0x1_0000
                                        + (((first - 0xd800) as u32) << 10)
                                        + (second as u32).saturating_sub(0xdc00);
                                    out.push(
                                        char::from_u32(combined)
                                            .ok_or_else(|| invalid("invalid surrogate pair"))?,
                                    );
                                } else {
                                    return Err(invalid("unpaired surrogate in string"));
                                }
                            } else {
                                out.push(
                                    char::from_u32(first as u32)
                                        .ok_or_else(|| invalid("invalid unicode escape"))?,
                                );
                            }
                        }
                        _ => return Err(invalid("invalid string escape")),
                    }
                }
                _ => {
                    // Re-decode the UTF-8 sequence starting here.
                    let rest = &self.bytes[self.index - 1..];
                    let text = std::str::from_utf8(rest)
                        .map_err(|_| invalid("invalid UTF-8 in string"))?;
                    let character = text.chars().next().ok_or_else(|| invalid("empty string"))?;
                    out.push(character);
                    self.index += character.len_utf8() - 1;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u16> {
        let raw = self
            .bytes
            .get(self.index..self.index + 4)
            .ok_or_else(|| invalid("short unicode escape"))?;
        let text = std::str::from_utf8(raw).map_err(|_| invalid("invalid unicode escape"))?;
        self.index += 4;
        u16::from_str_radix(text, 16).map_err(|_| invalid("invalid unicode escape"))
    }
}

/// SHA-1 (FIPS 180-1), needed only for UUIDv5 reply identities.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    let bit_length = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 80];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) =
            (state[0], state[1], state[2], state[3], state[4]);
        for (index, word) in words.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999_u32),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
    }
    let mut out = [0_u8; 20];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn uuid_bytes(text: &str) -> Result<[u8; 16]> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid(format!("request_id {text:?} is not a UUID")));
    }
    let mut out = [0_u8; 16];
    for index in 0..16 {
        out[index] = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| invalid("invalid UUID"))?;
    }
    Ok(out)
}

fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// UUIDv5 (SHA-1, RFC 4122) of `name` under the namespace UUID `base`.
fn uuid5(base: &str, name: &str) -> Result<String> {
    if name.is_empty() {
        return Ok(format_uuid(&uuid_bytes(base)?));
    }
    let namespace = uuid_bytes(base)?;
    let mut data = namespace.to_vec();
    data.extend_from_slice(name.as_bytes());
    let digest = sha1(&data);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format_uuid(&bytes))
}

fn reply_identity(record: &Value, index: i64) -> Result<String> {
    let base = record
        .get("request_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("request request_id must be a string"))?;
    if index == 0 {
        // Validate it is a UUID, then return it normalized like the other edition's UUID string.
        return Ok(format_uuid(&uuid_bytes(base)?));
    }
    uuid5(base, &format!("reply:{index}"))
}

fn valid_timestamp(value: &str) -> bool {
    // RFC3339 with timezone and at most nine fractional digits (the other edition's
    // `_RFC3339_TIMESTAMP` shape), with real calendar ranges.
    if value.len() > 128 {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    let digit = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let shape = digit(0)
        && digit(1)
        && digit(2)
        && digit(3)
        && bytes.get(4) == Some(&b'-')
        && digit(5)
        && digit(6)
        && bytes.get(7) == Some(&b'-')
        && digit(8)
        && digit(9)
        && matches!(bytes.get(10), Some(b'T') | Some(b't'))
        && digit(11)
        && digit(12)
        && bytes.get(13) == Some(&b':')
        && digit(14)
        && digit(15)
        && bytes.get(16) == Some(&b':')
        && digit(17)
        && digit(18);
    if !shape {
        return false;
    }
    let number =
        |start: usize, len: usize| -> Option<u32> { value.get(start..start + len)?.parse().ok() };
    let (month, day, hour, minute, second) = (
        number(5, 2).unwrap_or(0),
        number(8, 2).unwrap_or(0),
        number(11, 2).unwrap_or(0),
        number(14, 2).unwrap_or(0),
        number(17, 2).unwrap_or(0),
    );
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }
    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start || index - start > 9 {
            return false;
        }
    }
    match bytes.get(index) {
        Some(b'Z') | Some(b'z') => index + 1 == bytes.len(),
        Some(b'+') | Some(b'-') => {
            index + 6 == bytes.len()
                && digit(index + 1)
                && digit(index + 2)
                && bytes.get(index + 3) == Some(&b':')
                && digit(index + 4)
                && digit(index + 5)
        }
        _ => false,
    }
}

fn summary_count(record: &Value, field: &str) -> Result<i64> {
    let value = record.get(field).cloned().unwrap_or(Value::from(0));
    match value.as_i64() {
        Some(count) if (0..=MAX_REPLY_ITEMS).contains(&count) && value.is_i64() => Ok(count),
        _ => Err(invalid(format!(
            "saved {field} must be an integer between 0 and {MAX_REPLY_ITEMS}"
        ))),
    }
}

fn summary_bytes(record: &Value, field: &str) -> Result<i64> {
    let value = record.get(field).cloned().unwrap_or(Value::from(0));
    match value.as_i64() {
        Some(count)
            if value.is_i64() && (0..=MAX_REPLY_ITEMS * MAX_REPLY_BYTES).contains(&count) =>
        {
            Ok(count)
        }
        _ => Err(invalid(format!(
            "saved {field} is outside the durable reply byte range"
        ))),
    }
}

fn reply_summary(record: &Value) -> Result<(i64, i64, i64, i64)> {
    if record.get("reply_storage").and_then(Value::as_i64) != Some(2) {
        return Err(invalid("reply outbox migration was not completed"));
    }
    let count = summary_count(record, "reply_item_count")?;
    let sent = summary_count(record, "reply_sent_count")?;
    let total_bytes = summary_bytes(record, "reply_total_bytes")?;
    let pending_bytes = summary_bytes(record, "reply_pending_bytes")?;
    if sent > count || pending_bytes > total_bytes {
        return Err(invalid(
            "saved reply summary counts or bytes are inconsistent",
        ));
    }
    Ok((count, sent, total_bytes, pending_bytes))
}

pub(crate) fn request_path(state: &Path, key: &str) -> Result<PathBuf> {
    let valid = (12..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !valid {
        return Err(invalid(
            "request must be a hexadecimal key or unique prefix of at least 12 characters",
        ));
    }
    let directory = state.join("requests");
    let mut matches: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| invalid(format!("cannot read {}: {error}", directory.display())))?;
    for entry in entries {
        let entry = entry.map_err(|error| invalid(format!("cannot read requests: {error}")))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(key) && name.ends_with(".json") {
            matches.push(entry.path());
        }
    }
    matches.sort();
    if matches.len() != 1 {
        return Err(invalid(format!(
            "request prefix must identify exactly one saved request; found {}",
            matches.len()
        )));
    }
    Ok(matches.into_iter().next().unwrap_or_default())
}

fn reply_item_path(state: &Path, key: &str, index: i64, sent: bool) -> PathBuf {
    state
        .join("replies")
        .join("items")
        .join(&key[0..2])
        .join(&key[2..4])
        .join(key)
        .join(if sent { "history" } else { "pending" })
        .join(format!("{:04}", index / REPLY_BUCKET_SIZE))
        .join(format!("{index:06}.json"))
}

pub(crate) fn read_json_value(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| invalid(format!("cannot read {}: {error}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|error| invalid(format!("cannot parse {}: {error}", path.display())))
}

fn get_text<'a>(item: &'a Ordered, field: &str, label: &str) -> Result<&'a str> {
    item.get(field)
        .and_then(Ordered::as_text)
        .ok_or_else(|| invalid(format!("{label} {field} must be a string")))
}

fn validate_reply_item(record: &Value, item: &Ordered, index: i64, space: &str) -> Result<()> {
    let fields = match item {
        Ordered::Object(fields) => fields,
        _ => return Err(invalid("reply item must be an object")),
    };
    let mut unknown: Vec<&str> = fields
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !ALLOWED_ITEM_FIELDS.contains(name))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(invalid(format!(
            "reply artifact contains unsupported fields: {}",
            unknown.join(", ")
        )));
    }
    if item.get("reply_key").and_then(Ordered::as_text) != Some(index.to_string().as_str()) {
        return Err(invalid(
            "reply artifact key does not match its sequence path",
        ));
    }
    if get_text(item, "request_id", "reply item")? != reply_identity(record, index)? {
        return Err(invalid(
            "reply artifact request ID does not match its stable sequence identity",
        ));
    }
    let text = get_text(item, "text", "reply item")?;
    if text.trim().is_empty() || text.len() > MAX_REPLY_BYTES as usize {
        return Err(invalid(
            "reply artifact text must contain 1-30000 UTF-8 bytes",
        ));
    }
    let source = get_text(item, "reply_source", "reply item")?;
    if !matches!(source, "file" | "capture" | "legacy") {
        return Err(invalid(
            "reply artifact source must be file, capture, or legacy",
        ));
    }
    let ordinal = item.get("reply_ordinal");
    if let Some(ordinal) = ordinal {
        if !matches!(ordinal, Ordered::Null)
            && !matches!(ordinal.as_int(), Some(value) if (1..=999_999).contains(&value))
        {
            return Err(invalid(
                "reply artifact ordinal must be between 1 and 999999",
            ));
        }
    }
    let shift = item.get("reply_shift_from");
    let shift_valid = match shift {
        None | Some(Ordered::Null) => true,
        Some(_) => {
            shift.and_then(Ordered::as_int) == Some(0)
                && index == 1
                && source == "capture"
                && ordinal.and_then(Ordered::as_int) == Some(1)
        }
    };
    if !shift_valid {
        return Err(invalid(
            "reply shift journal has invalid transaction metadata",
        ));
    }
    let identifier = item.get("reply_id");
    if let Some(identifier) = identifier {
        if !matches!(identifier, Ordered::Null) {
            let identifier = identifier
                .as_text()
                .ok_or_else(|| invalid("reply artifact provider ID must be a string"))?;
            let prefix = format!("{space}/messages/");
            let valid = identifier.starts_with(&prefix)
                && identifier[prefix.len()..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
                && identifier.len() > prefix.len();
            if !valid {
                return Err(invalid(
                    "reply artifact provider ID belongs outside the configured space",
                ));
            }
        }
    }
    let replied_at = item.get("replied_at");
    if let Some(replied_at) = replied_at {
        if !matches!(replied_at, Ordered::Null) {
            let replied_at = replied_at
                .as_text()
                .ok_or_else(|| invalid("reply artifact completion time must be a string"))?;
            if !valid_timestamp(replied_at) {
                return Err(invalid(
                    "chat timestamps must be RFC3339 with a timezone and at most nine fractional digits",
                ));
            }
        }
    }
    let id_missing = matches!(identifier, None | Some(Ordered::Null));
    let at_missing = matches!(replied_at, None | Some(Ordered::Null));
    if id_missing != at_missing {
        return Err(invalid(
            "reply artifact provider ID and completion time must appear together",
        ));
    }
    Ok(())
}

/// Read every retained reply body for one request; returns the JSON document
/// the other edition's `chat history` prints (without the trailing newline).
pub fn read_reply_history(state: &Path, key: &str) -> Result<String> {
    let bridge = read_json_value(&state.join("bridge.json"))?;
    let space = bridge
        .get("config")
        .and_then(|config| config.get("space"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("saved config space must be a string"))?
        .to_owned();
    let lock_path = state.join(".bridge.lock");
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| invalid(format!("cannot open chat bridge lock: {error}")))?;
    FileExt::lock_shared(&lock_file)
        .map_err(|error| invalid(format!("cannot lock chat bridge lock: {error}")))?;
    let path = request_path(state, key)?;
    let record_text = std::fs::read_to_string(&path)
        .map_err(|error| invalid(format!("cannot read {}: {error}", path.display())))?;
    let record: Value = serde_json::from_str(&record_text)
        .map_err(|error| invalid(format!("cannot parse {}: {error}", path.display())))?;
    if record.get("reply_storage").and_then(Value::as_i64) != Some(2) {
        return Err(invalid(
            "reply history requires chat tick/run migration first",
        ));
    }
    let (count, sent, total_bytes, pending_bytes) = reply_summary(&record)?;
    let record_key = record
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("request key must be a string"))?;
    let mut items = Vec::new();
    for index in 0..count {
        let item_path = reply_item_path(state, record_key, index, index < sent);
        let item_text = std::fs::read_to_string(&item_path)
            .map_err(|error| invalid(format!("cannot read {}: {error}", item_path.display())))?;
        let item = Parser::parse(&item_text)?;
        validate_reply_item(&record, &item, index, &space)?;
        items.push(item);
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned();
    let document = Ordered::Object(vec![
        ("request".to_owned(), Ordered::Text(stem)),
        ("count".to_owned(), Ordered::Int(count)),
        ("sent".to_owned(), Ordered::Int(sent)),
        ("total_bytes".to_owned(), Ordered::Int(total_bytes)),
        ("pending_bytes".to_owned(), Ordered::Int(pending_bytes)),
        ("items".to_owned(), Ordered::Array(items)),
    ]);
    let mut out = String::new();
    document.dump(0, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_and_uuid5_match_python_vectors() {
        // Other edition: uuid.uuid5(uuid.UUID("12345678-1234-5678-1234-567812345678"), "reply:1")
        // and uuid.uuid5(uuid.NAMESPACE_DNS, "python.org") are published shapes;
        // the namespace vector below is the canonical published example.
        let dns = uuid5("6ba7b810-9dad-11d1-80b4-00c04fd430c8", "python.org").expect("uuid5");
        assert_eq!(dns, "886313e1-3b8a-5372-9b90-0c9aee199e5d");
        let identity = reply_identity(
            &serde_json::json!({"request_id": "12345678-1234-5678-1234-567812345678"}),
            1,
        )
        .expect("identity");
        assert_eq!(identity.len(), 36);
        assert_eq!(&identity[14..15], "5");
    }

    #[test]
    fn history_reads_python_format_state() {
        let directory = tempfile_dir();
        let state = directory.join("state");
        let key = "a".repeat(64);
        let request_id = "12345678-1234-5678-1234-567812345678";
        std::fs::create_dir_all(state.join("requests")).expect("requests");
        std::fs::write(
            state.join("bridge.json"),
            r#"{"config": {"space": "spaces/test"}}"#,
        )
        .expect("bridge");
        std::fs::write(
            state.join("requests").join(format!("{key}.json")),
            serde_json::json!({
                "key": key,
                "request_id": request_id,
                "reply_storage": 2,
                "reply_item_count": 1,
                "reply_sent_count": 0,
                "reply_total_bytes": 5,
                "reply_pending_bytes": 5
            })
            .to_string(),
        )
        .expect("request");
        let item_path = reply_item_path(&state, &key, 0, false);
        std::fs::create_dir_all(item_path.parent().expect("parent")).expect("items");
        std::fs::write(
            &item_path,
            serde_json::json!({
                "reply_key": "0",
                "request_id": request_id,
                "reply_source": "capture",
                "text": "hello",
                "reply_ordinal": 1,
                "reply_shift_from": null,
                "reply_id": null,
                "replied_at": null
            })
            .to_string(),
        )
        .expect("item");
        let output = read_reply_history(&state, &key[..12]).expect("history");
        assert!(output.contains("\"text\": \"hello\""), "{output}");
        assert!(output.contains("\"count\": 1"), "{output}");
        let bad = read_reply_history(&state, "zzzzzzzzzzzz");
        assert!(bad.is_err());
    }

    fn tempfile_dir() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("agentctl-chat-history-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("tempdir");
        directory
    }
}
