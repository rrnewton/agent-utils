//! Stored-line expansion: codec lines to logical events.
//!
//! Expansion happens before union and before any event check, once per
//! physical copy, and any failure refuses the whole read. Ordinary lines
//! parse as written. Stored lines (carrying `legacy_row_codec`) are strict:
//! no repeated key anywhere, integer tokens both decoders read alike, and
//! evidence authenticated before it becomes a row.

use crate::digest::sha256_of;
use crate::event::{RawEvent, Refusal, SCHEMA};
use crate::pyjson::{dumps_sorted, parse, parse_with, ParseOptions, PyValue};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Maximum raw evidence row size (48 MiB).
pub const MAX_ROW_BYTES: u64 = 48 * 1024 * 1024;
/// Maximum compressed evidence object size (49 MiB).
pub const MAX_COMPRESSED_BYTES: u64 = MAX_ROW_BYTES + 1024 * 1024;

const CODEC_KEY: &str = "legacy_row_codec";
const CELL_DEDUP_CODEC: &str = "cell-dedup/v1";
const ROW_REF_CODEC: &str = "row-ref/v1";
const CELL_IDENTITY_KEYS: [&str; 5] = ["backend", "category", "lane", "mode", "test"];
const CODEC_FIELDS: [&str; 5] = [
    "census_cells_indexed",
    "codec",
    "comparisons",
    "evidence_binding_absent",
    "selected_from_cells",
];
const ROW_REF_FIELDS: [&str; 6] = [
    "bytes",
    "codec",
    "compressed_bytes",
    "compressed_sha256",
    "locator",
    "sha256",
];

/// Expansion settings whose values are deployment vocabulary, not core
/// constants.
#[derive(Clone, Debug)]
pub struct ExpandConfig {
    /// `cell_results` field naming the validated tree, copied into a
    /// derived evidence binding as `tree`.
    pub binding_tree_field: String,
}

impl Default for ExpandConfig {
    fn default() -> Self {
        Self {
            binding_tree_field: "tree".to_owned(),
        }
    }
}

/// Compressed evidence bytes by locator, behind a size cap.
pub trait EvidenceSource {
    /// Read at most `max_bytes + 1` compressed bytes for `locator`.
    fn read_compressed(&self, locator: &str, max_bytes: u64) -> Result<Vec<u8>, Refusal>;
}

/// Evidence source that refuses every fetch (corpora without row-refs).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoEvidence;

impl EvidenceSource for NoEvidence {
    fn read_compressed(&self, _locator: &str, _max_bytes: u64) -> Result<Vec<u8>, Refusal> {
        Err(Refusal::EvidenceRefused {
            reason: "no evidence source configured".to_owned(),
        })
    }
}

/// Filesystem evidence archive rooted at one directory.
#[derive(Clone, Debug)]
pub struct FileEvidence {
    root: PathBuf,
}

impl FileEvidence {
    /// Root the archive at `root` (`<root>/<locator>` is the object path).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl EvidenceSource for FileEvidence {
    fn read_compressed(&self, locator: &str, max_bytes: u64) -> Result<Vec<u8>, Refusal> {
        let refused = |reason: String| Refusal::EvidenceRefused { reason };
        if Path::new(locator).is_absolute()
            || locator
                .split('/')
                .any(|part| part == ".." || part.is_empty())
        {
            return Err(refused(
                "evidence locator is not a relative path".to_owned(),
            ));
        }
        let path = self.root.join(locator);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| refused(format!("retained evidence is unreadable: {e}")))?;
        if !metadata.file_type().is_file() {
            return Err(refused(
                "retained evidence is not a regular file".to_owned(),
            ));
        }
        let mut file = std::fs::File::open(&path)
            .map_err(|e| refused(format!("retained evidence is unreadable: {e}")))?;
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(max_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| refused(format!("retained evidence is unreadable: {e}")))?;
        Ok(bytes)
    }
}

fn malformed(reason: String) -> Refusal {
    Refusal::Malformed { reason }
}

fn obj_get<'a>(value: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    value.get(key)
}

fn obj_pairs_mut(value: &mut PyValue) -> Option<&mut Vec<(String, PyValue)>> {
    match value {
        PyValue::Object(pairs) => Some(pairs),
        _ => None,
    }
}

fn obj_set(value: &mut PyValue, key: &str, child: PyValue) {
    if let Some(pairs) = obj_pairs_mut(value) {
        if let Some(slot) = pairs.iter_mut().rev().find(|(k, _)| k == key) {
            slot.1 = child;
        } else {
            pairs.push((key.to_owned(), child));
        }
    }
}

fn obj_remove(value: &mut PyValue, key: &str) {
    if let Some(pairs) = obj_pairs_mut(value) {
        pairs.retain(|(k, _)| k != key);
    }
}

fn is_int(value: &PyValue) -> bool {
    value.is_int()
}

/// Whether an integer token fits `[-2^63, 2^64)` and is not `-0`.
fn int_token_in_range(token: &str) -> bool {
    if token == "-0" {
        return false;
    }
    let (negative, digits) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let limit = if negative {
        "9223372036854775808"
    } else {
        "18446744073709551615"
    };
    digits.len() < limit.len() || (digits.len() == limit.len() && digits <= limit)
}

/// Refuse when any integer token outside strings is not one both decoders
/// read as an integer.
fn check_integer_tokens(line: &str) -> Result<(), Refusal> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                if bytes[i] == b'-' {
                    i += 1;
                }
                let mut is_float = false;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'.' {
                    is_float = true;
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                    is_float = true;
                    i += 1;
                    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                        i += 1;
                    }
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if !is_float {
                    let token = &line[start..i];
                    if !int_token_in_range(token) {
                        return Err(malformed(format!(
                            "stored {CELL_DEDUP_CODEC} line is malformed: integer {token} is not one both decoders read as an integer"
                        )));
                    }
                }
            }
            _ => i += 1,
        }
    }
    Ok(())
}

fn codec_refuse(reason: &str) -> Refusal {
    malformed(format!(
        "stored {CELL_DEDUP_CODEC} line is malformed: {reason}"
    ))
}

/// Complete five-key cell identity, when every key holds a string.
fn cell_identity(cell: &PyValue) -> Option<PyValue> {
    let mut pairs = Vec::with_capacity(5);
    for key in CELL_IDENTITY_KEYS {
        let value = cell.get(key)?.as_str()?;
        pairs.push((key.to_owned(), PyValue::Str(value.to_owned())));
    }
    Some(PyValue::Object(pairs))
}

fn expand_cell_dedup(mut event: PyValue, config: &ExpandConfig) -> Result<PyValue, Refusal> {
    let descriptor = obj_get(&event, CODEC_KEY).cloned().unwrap_or(PyValue::Null);
    obj_remove(&mut event, CODEC_KEY);
    let Some(descriptor_pairs) = descriptor.as_object() else {
        return Err(codec_refuse("codec descriptor has the wrong fields"));
    };
    let mut fields: Vec<&str> = descriptor_pairs.iter().map(|(k, _)| k.as_str()).collect();
    fields.sort_unstable();
    let mut expected: Vec<&str> = CODEC_FIELDS.to_vec();
    expected.sort_unstable();
    if fields != expected {
        return Err(codec_refuse("codec descriptor has the wrong fields"));
    }
    if descriptor.get("codec").and_then(PyValue::as_str) != Some(CELL_DEDUP_CODEC) {
        return Err(codec_refuse("unknown codec"));
    }
    let Some(PyValue::Array(comparisons)) = descriptor.get("comparisons").cloned() else {
        return Err(codec_refuse("codec tables are not lists"));
    };
    let Some(PyValue::Array(absent_raw)) = descriptor.get("evidence_binding_absent").cloned()
    else {
        return Err(codec_refuse("codec tables are not lists"));
    };
    let mut absent: Vec<usize> = Vec::with_capacity(absent_raw.len());
    for entry in &absent_raw {
        if !is_int(entry) {
            return Err(codec_refuse("evidence_binding_absent holds a non-integer"));
        }
        let Some(index) = entry.as_u64().and_then(|n| usize::try_from(n).ok()) else {
            return Err(codec_refuse(
                "evidence_binding_absent names a cell twice or a cell that does not exist",
            ));
        };
        absent.push(index);
    }
    let (Some(PyValue::Bool(selected_from_cells)), Some(PyValue::Bool(census_indexed))) = (
        descriptor.get("selected_from_cells").cloned(),
        descriptor.get("census_cells_indexed").cloned(),
    ) else {
        return Err(codec_refuse("codec flags are not booleans"));
    };
    if obj_get(&event, "schema").and_then(PyValue::as_str) != Some(SCHEMA) {
        return Err(codec_refuse("event is not a validate-ledger/v1 event"));
    }
    // Navigate to legacy_row.cell_results.cells, mutating a clone of the row.
    let Some(mut row) = obj_get(&event, "legacy_row").cloned() else {
        return Err(codec_refuse(
            "legacy_row.cell_results.cells is not a list of objects",
        ));
    };
    let Some(mut cell_results) = obj_get(&row, "cell_results").cloned() else {
        return Err(codec_refuse(
            "legacy_row.cell_results.cells is not a list of objects",
        ));
    };
    let Some(PyValue::Array(mut cells)) = obj_get(&cell_results, "cells").cloned() else {
        return Err(codec_refuse(
            "legacy_row.cell_results.cells is not a list of objects",
        ));
    };
    if cells.iter().any(|cell| !cell.is_object()) {
        return Err(codec_refuse(
            "legacy_row.cell_results.cells is not a list of objects",
        ));
    }
    let mut absent_seen: Vec<usize> = Vec::new();
    for index in &absent {
        if *index >= cells.len() || absent_seen.contains(index) {
            return Err(codec_refuse(
                "evidence_binding_absent names a cell twice or a cell that does not exist",
            ));
        }
        absent_seen.push(*index);
    }
    for (index, cell) in cells.iter_mut().enumerate() {
        if let Some(verdict) = obj_get(cell, "cell_verdict").cloned() {
            if verdict.is_object() {
                if let Some(slot_value) = verdict.get("comparison") {
                    if is_int(slot_value) {
                        let Some(slot) = slot_value.as_u64().and_then(|n| usize::try_from(n).ok())
                        else {
                            return Err(codec_refuse("comparison index is out of range"));
                        };
                        let Some(replacement) = comparisons.get(slot).cloned() else {
                            return Err(codec_refuse(&format!(
                                "comparison index {slot} is out of range"
                            )));
                        };
                        let mut verdict = verdict;
                        obj_set(&mut verdict, "comparison", replacement);
                        obj_set(cell, "cell_verdict", verdict);
                    }
                }
            }
        }
        let listed_absent = absent.contains(&index);
        let has_attempt = cell.contains_key("selected_attempt");
        let has_binding = cell.contains_key("evidence_binding");
        if has_attempt && !has_binding && !listed_absent {
            let binding = derived_binding(cell, &cell_results, config).ok_or_else(|| {
                codec_refuse(&format!("cell {index} has no derivable evidence binding"))
            })?;
            obj_set(cell, "evidence_binding", binding);
        } else if listed_absent && (!has_attempt || has_binding) {
            return Err(codec_refuse(&format!(
                "cell {index} is listed without an evidence binding but is not such a cell"
            )));
        }
    }
    if selected_from_cells {
        if cell_results.contains_key("selected") {
            return Err(codec_refuse("selected is both stored and derived"));
        }
        let mut selected = Vec::with_capacity(cells.len());
        for (slot, cell) in cells.iter().enumerate() {
            let Some(identity) = cell_identity(cell) else {
                return Err(codec_refuse(&format!(
                    "cell {slot} has no complete identity"
                )));
            };
            selected.push(identity);
        }
        obj_set(&mut cell_results, "selected", PyValue::Array(selected));
    }
    obj_set(&mut cell_results, "cells", PyValue::Array(cells.clone()));
    if census_indexed {
        let Some(mut census) = obj_get(&row, "raw_result_input_census_v1").cloned() else {
            return Err(codec_refuse("census is indexed but has no file list"));
        };
        let Some(PyValue::Array(mut files)) = obj_get(&census, "files").cloned() else {
            return Err(codec_refuse("census is indexed but has no file list"));
        };
        for file in files.iter_mut() {
            let Some(PyValue::Array(mut census_rows)) = obj_get(file, "rows").cloned() else {
                continue;
            };
            for census_row in census_rows.iter_mut() {
                if !census_row.is_object() {
                    continue;
                }
                if let Some(slot_value) = obj_get(census_row, "cell").cloned() {
                    if is_int(&slot_value) {
                        let Some(slot) = slot_value.as_u64().and_then(|n| usize::try_from(n).ok())
                        else {
                            return Err(codec_refuse("cell index is out of range"));
                        };
                        let Some(identity) = cells.get(slot).and_then(cell_identity) else {
                            return Err(codec_refuse(&format!(
                                "cell index {slot} is out of range"
                            )));
                        };
                        obj_set(census_row, "cell", identity);
                    }
                }
            }
            obj_set(file, "rows", PyValue::Array(census_rows));
        }
        obj_set(&mut census, "files", PyValue::Array(files));
        obj_set(&mut row, "raw_result_input_census_v1", census);
    }
    obj_set(&mut row, "cell_results", cell_results);
    obj_set(&mut event, "legacy_row", row);
    Ok(event)
}

fn derived_binding(
    cell: &PyValue,
    cell_results: &PyValue,
    config: &ExpandConfig,
) -> Option<PyValue> {
    let run_id = cell_results.get("run_id")?.as_str()?;
    let tree = cell_results.get(&config.binding_tree_field)?.as_str()?;
    let attempt = cell.get("selected_attempt")?;
    if !is_int(attempt) {
        return None;
    }
    // Attempt must fit the shared integer range.
    match attempt {
        PyValue::Int(text) if int_token_in_range(text) => {}
        _ => return None,
    }
    let identity = cell_identity(cell)?;
    let field = |key: &str| identity.get(key).and_then(PyValue::as_str).unwrap_or("");
    let backend = field("backend");
    let mode = field("mode");
    let backend = if backend.is_empty() && mode == "naked" {
        "native"
    } else {
        backend
    };
    let series_cell = format!("{}/{}/{}", field("test"), mode, backend);
    Some(PyValue::Object(vec![
        ("producer".to_owned(), PyValue::Str("validate".to_owned())),
        ("run_id".to_owned(), PyValue::Str(run_id.to_owned())),
        ("selected_attempt".to_owned(), attempt.clone()),
        ("series_cell".to_owned(), PyValue::Str(series_cell)),
        ("tree".to_owned(), PyValue::Str(tree.to_owned())),
    ]))
}

fn row_ref_refuse(reason: &str) -> Refusal {
    malformed(format!(
        "stored {ROW_REF_CODEC} line is malformed: {reason}"
    ))
}

fn is_lower_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn bounded_size(reference: &PyValue, field: &str, bound: u64) -> Result<u64, Refusal> {
    match reference.get(field) {
        Some(value) if is_int(value) => match value.as_u64() {
            Some(size) if size > 0 && size <= bound => Ok(size),
            _ => Err(row_ref_refuse(&format!(
                "reference {field} is not a positive bounded integer"
            ))),
        },
        _ => Err(row_ref_refuse(&format!(
            "reference {field} is not a positive bounded integer"
        ))),
    }
}

fn expand_row_ref(mut event: PyValue, evidence: &dyn EvidenceSource) -> Result<PyValue, Refusal> {
    let reference = obj_get(&event, CODEC_KEY).cloned().unwrap_or(PyValue::Null);
    let Some(reference_pairs) = reference.as_object() else {
        return Err(row_ref_refuse("reference has missing or unknown fields"));
    };
    let mut fields: Vec<&str> = reference_pairs.iter().map(|(k, _)| k.as_str()).collect();
    fields.sort_unstable();
    let mut expected: Vec<&str> = ROW_REF_FIELDS.to_vec();
    expected.sort_unstable();
    if fields != expected {
        return Err(row_ref_refuse("reference has missing or unknown fields"));
    }
    if reference.get("codec").and_then(PyValue::as_str) != Some(ROW_REF_CODEC) {
        return Err(row_ref_refuse("reference has an unsupported codec"));
    }
    let digest_of = |field: &str| -> Result<String, Refusal> {
        match reference.get(field).and_then(PyValue::as_str) {
            Some(text) if is_lower_hex64(text) => Ok(text.to_owned()),
            _ => Err(row_ref_refuse(&format!(
                "reference {field} is not lowercase sha256"
            ))),
        }
    };
    let sha256 = digest_of("sha256")?;
    let compressed_sha256 = digest_of("compressed_sha256")?;
    let bytes = bounded_size(&reference, "bytes", MAX_ROW_BYTES)?;
    let compressed_bytes = bounded_size(&reference, "compressed_bytes", MAX_COMPRESSED_BYTES)?;
    let Some(locator) = reference.get("locator").and_then(PyValue::as_str) else {
        return Err(row_ref_refuse(
            "reference locator is not its immutable host/content address",
        ));
    };
    let Some(locator_host) = parse_locator(locator, &sha256) else {
        return Err(row_ref_refuse(
            "reference locator is not its immutable host/content address",
        ));
    };
    if obj_get(&event, "schema").and_then(PyValue::as_str) != Some(SCHEMA)
        || obj_get(&event, "event_type").and_then(PyValue::as_str) != Some("run.result")
        || obj_get(&event, "host").and_then(PyValue::as_str) != Some(locator_host.as_str())
    {
        return Err(row_ref_refuse(
            "reference does not name this run.result host",
        ));
    }
    if event.contains_key("legacy_row") {
        return Err(row_ref_refuse(
            "line has both a full row and a row reference",
        ));
    }
    let compressed = evidence.read_compressed(locator, compressed_bytes)?;
    if compressed.len() as u64 != compressed_bytes || sha256_of(&compressed) != compressed_sha256 {
        return Err(Refusal::EvidenceRefused {
            reason: "retained evidence compressed sha256 differs".to_owned(),
        });
    }
    let raw = zstd_decompress_bounded(&compressed, bytes)?;
    if raw.len() as u64 != bytes || sha256_of(&raw) != sha256 {
        return Err(Refusal::EvidenceRefused {
            reason: "retained evidence row size or sha256 differs".to_owned(),
        });
    }
    let raw_text = String::from_utf8(raw).map_err(|_| Refusal::EvidenceRefused {
        reason: "retained evidence is not a full JSON row".to_owned(),
    })?;
    let row = parse_with(
        &raw_text,
        ParseOptions {
            unique_keys: true,
            finite_only: true,
        },
    )
    .map_err(|e| Refusal::EvidenceRefused {
        reason: format!("retained evidence is not an unambiguous JSON row: {e}"),
    })?;
    if !row.is_object() {
        return Err(Refusal::EvidenceRefused {
            reason: "retained evidence is not a full JSON row".to_owned(),
        });
    }
    obj_remove(&mut event, CODEC_KEY);
    obj_set(&mut event, "legacy_row", row);
    Ok(event)
}

fn parse_locator(locator: &str, sha256: &str) -> Option<String> {
    let rest = locator.strip_prefix("run-evidence/")?;
    let (host, file) = rest.split_once('/')?;
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || file.contains('/')
    {
        return None;
    }
    let hex = file.strip_suffix(".json.zst")?;
    if hex == sha256 && is_lower_hex64(hex) {
        Some(host.to_owned())
    } else {
        None
    }
}

fn zstd_decompress_bounded(compressed: &[u8], max_output: u64) -> Result<Vec<u8>, Refusal> {
    let refused = |reason: String| Refusal::EvidenceRefused { reason };
    let mut child = Command::new("zstd")
        .args(["--quiet", "--decompress", "--stdout", "--memory=128MB"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| refused(format!("retained evidence decompressor unavailable: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(compressed);
    }
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut out = Vec::new();
    stdout
        .by_ref()
        .take(max_output + 1)
        .read_to_end(&mut out)
        .map_err(|e| refused(format!("retained evidence is unreadable: {e}")))?;
    if out.len() as u64 > max_output {
        let _ = child.kill();
        let _ = child.wait();
        return Err(refused(
            "retained evidence expands past its declared size".to_owned(),
        ));
    }
    let status = child
        .wait()
        .map_err(|e| refused(format!("retained evidence is unreadable: {e}")))?;
    if !status.success() {
        return Err(refused(
            "retained evidence is not a valid zstd object".to_owned(),
        ));
    }
    Ok(out)
}

/// Expand one stored or ordinary line into a logical event.
pub fn expand_line(line: &str, evidence: &dyn EvidenceSource) -> Result<RawEvent, Refusal> {
    expand_line_with(line, evidence, &ExpandConfig::default())
}

/// Expand one line with explicit expansion settings.
pub fn expand_line_with(
    line: &str,
    evidence: &dyn EvidenceSource,
    config: &ExpandConfig,
) -> Result<RawEvent, Refusal> {
    let trimmed = line.trim();
    let probe = parse(trimmed).map_err(|e| malformed(format!("line is not valid JSON: {e}")))?;
    if !probe.is_object() {
        return Err(malformed("line is not a JSON object".to_owned()));
    }
    if !probe.contains_key(CODEC_KEY) {
        return Ok(RawEvent {
            line: trimmed.to_owned(),
            value: probe,
        });
    }
    let strict = parse_with(
        trimmed,
        ParseOptions {
            unique_keys: true,
            finite_only: false,
        },
    )
    .map_err(|e| malformed(format!("stored {CELL_DEDUP_CODEC} line is malformed: {e}")))?;
    let codec = strict
        .get(CODEC_KEY)
        .and_then(|descriptor| descriptor.get("codec"))
        .and_then(PyValue::as_str)
        .unwrap_or("")
        .to_owned();
    let expanded = if codec == ROW_REF_CODEC {
        expand_row_ref(strict, evidence)?
    } else {
        check_integer_tokens(trimmed)?;
        expand_cell_dedup(strict, config)?
    };
    Ok(RawEvent {
        line: dumps_sorted(&expanded),
        value: expanded,
    })
}

/// Split text with Python `str.splitlines` boundaries.
fn splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\n' => {
                lines.push(std::mem::take(&mut current));
            }
            '\r' => {
                lines.push(std::mem::take(&mut current));
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\u{000B}' | '\u{000C}' | '\u{001C}' | '\u{001D}' | '\u{001E}' | '\u{0085}'
            | '\u{2028}' | '\u{2029}' => {
                lines.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Read one shard's text into expanded events, in file order.
pub fn read_shard_text(
    text: &str,
    evidence: &dyn EvidenceSource,
) -> Result<Vec<RawEvent>, Refusal> {
    read_shard_text_with(text, evidence, &ExpandConfig::default())
}

/// Read one shard's text with explicit expansion settings.
pub fn read_shard_text_with(
    text: &str,
    evidence: &dyn EvidenceSource,
    config: &ExpandConfig,
) -> Result<Vec<RawEvent>, Refusal> {
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(malformed(
            "final line has no newline terminator — the shard is truncated".to_owned(),
        ));
    }
    let mut events = Vec::new();
    for line in splitlines(text) {
        if line.trim().is_empty() {
            continue;
        }
        events.push(expand_line_with(&line, evidence, config)?);
    }
    Ok(events)
}
