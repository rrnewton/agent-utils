//! The seekable JSONL writer: multi-member gzip plus its
//! sidecar index, at byte parity with
//! `py/wrkviz/seekable_jsonl.py`'s write path (the reader side
//! already lives with the query implementation).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::Digest as _;

use crate::json::dump_compact_ordered;

const INDEX_FORMAT: &str = "wrkviz/seekable-jsonl-index";
const INDEX_SUFFIX: &str = ".index.jsonl";
const DEFAULT_TARGET_CHUNK_BYTES: u64 = 1 << 20;

/// One member's index entry.
#[derive(Clone, Debug)]
pub struct ChunkIndexEntry {
    /// Compressed offset.
    pub c_off: u64,
    /// Compressed length.
    pub c_len: u64,
    /// Uncompressed offset.
    pub u_off: u64,
    /// Uncompressed length.
    pub u_len: u64,
    /// First line number.
    pub l0: u64,
    /// Line count.
    pub n: u64,
    /// Earliest timestamp.
    pub t0: Option<i64>,
    /// Latest timestamp.
    pub t1: Option<i64>,
}

impl ChunkIndexEntry {
    fn to_json(&self) -> Value {
        let mut entry = serde_json::Map::new();
        entry.insert("c_off".into(), self.c_off.into());
        entry.insert("c_len".into(), self.c_len.into());
        entry.insert("u_off".into(), self.u_off.into());
        entry.insert("u_len".into(), self.u_len.into());
        entry.insert("l0".into(), self.l0.into());
        entry.insert("n".into(), self.n.into());
        entry.insert("t0".into(), self.t0.map_or(Value::Null, Value::from));
        entry.insert("t1".into(), self.t1.map_or(Value::Null, Value::from));
        Value::Object(entry)
    }
}

/// A rendered sidecar index.
#[derive(Clone, Debug)]
pub struct ChunkIndex {
    /// Record count.
    pub record_count: u64,
    /// Compressed size.
    pub c_size: u64,
    /// Uncompressed size.
    pub u_size: u64,
    /// Compressed digest.
    pub c_sha256: String,
    /// Uncompressed digest.
    pub u_sha256: String,
    /// Members.
    pub members: Vec<ChunkIndexEntry>,
    target_chunk_bytes: u64,
    timestamp_key: String,
    timestamps_sorted: bool,
    data_file: String,
}

impl ChunkIndex {
    fn to_text(&self) -> String {
        let mut header = serde_json::Map::new();
        header.insert("format".into(), Value::String(INDEX_FORMAT.to_string()));
        header.insert("version".into(), 1.into());
        header.insert("codec".into(), Value::String("gzip".to_string()));
        header.insert("codec_level".into(), 6.into());
        header.insert("target_chunk_bytes".into(), self.target_chunk_bytes.into());
        header.insert(
            "timestamp_key".into(),
            Value::String(self.timestamp_key.clone()),
        );
        header.insert("timestamps_sorted".into(), self.timestamps_sorted.into());
        header.insert("member_count".into(), (self.members.len() as u64).into());
        header.insert("record_count".into(), self.record_count.into());
        header.insert("c_size".into(), self.c_size.into());
        header.insert("u_size".into(), self.u_size.into());
        header.insert("c_sha256".into(), Value::String(self.c_sha256.clone()));
        header.insert("u_sha256".into(), Value::String(self.u_sha256.clone()));
        header.insert("data_file".into(), Value::String(self.data_file.clone()));
        let mut text = dump_compact_ordered(&Value::Object(header));
        text.push('\n');
        for member in &self.members {
            text.push_str(&dump_compact_ordered(&member.to_json()));
            text.push('\n');
        }
        text
    }
}

fn bisectable(members: &[ChunkIndexEntry]) -> bool {
    let mut previous: Option<i64> = None;
    for entry in members {
        let (Some(t0), Some(t1)) = (entry.t0, entry.t1) else {
            return false;
        };
        if let Some(previous) = previous {
            if t0 < previous {
                return false;
            }
        }
        previous = Some(t1);
    }
    true
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A reproducible gzip-6 stream with no filename or wall-clock
/// timestamp (CPython `gzip` with `mtime=0`, OS byte 255).
pub fn deterministic_gzip(data: &[u8]) -> Vec<u8> {
    let header = flate2::GzBuilder::new().mtime(0).operating_system(255);
    let mut encoder = header.write(Vec::new(), flate2::Compression::new(6));
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// What `write_seekable_jsonl` did.
pub struct WriteReport {
    /// Data path.
    pub path: PathBuf,
    /// Index path.
    pub index_path: PathBuf,
    /// The index.
    pub index: ChunkIndex,
    /// Whether data changed.
    pub data_changed: bool,
    /// Whether the index changed.
    pub index_changed: bool,
}

/// The conventional sidecar path for a chunked JSONL file.
pub fn index_path_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}{INDEX_SUFFIX}"))
}

fn timestamp_of(record: &Value, key: &str, number: usize) -> Result<Option<i64>, String> {
    let where_ = format!("record {number}");
    match record.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) if value.is_i64() || value.is_u64() => Ok(value.as_i64()),
        Some(_) => Err(format!("{where_}: {key} is not an integer")),
    }
}

/// Write records as chunked JSONL, encoding each one canonically.
pub fn write_seekable_jsonl(
    path: &Path,
    records: &[Value],
    timestamp_key: &str,
) -> Result<WriteReport, String> {
    let sidecar = index_path_for(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| crate::json::os_error(&error, parent))?;
    }
    let mut encoded: Vec<(Vec<u8>, Option<i64>)> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        // Compact separators and sorted keys, `allow_nan=False`.
        if crate::json::has_nonfinite(record) {
            return Err(format!("record {index}: non-finite number"));
        }
        let line = crate::json::dump_compact(record);
        if line.contains('\n') {
            return Err(format!("record {index}: encoded record contains a newline"));
        }
        encoded.push((
            line.into_bytes(),
            timestamp_of(record, timestamp_key, index)?,
        ));
    }
    write_encoded(path, &sidecar, &encoded, timestamp_key)
}

struct FlushState {
    members: Vec<ChunkIndexEntry>,
    compressed: Vec<u8>,
    c_hash: sha2::Sha256,
    u_hash: sha2::Sha256,
    c_off: u64,
    u_off: u64,
    line_no: u64,
}

impl FlushState {
    fn flush(&mut self, pending: &[u8], pending_lines: u64, t0: Option<i64>, t1: Option<i64>) {
        let member = deterministic_gzip(pending);
        self.c_hash.update(&member);
        self.u_hash.update(pending);
        self.members.push(ChunkIndexEntry {
            c_off: self.c_off,
            c_len: member.len() as u64,
            u_off: self.u_off,
            u_len: pending.len() as u64,
            l0: self.line_no,
            n: pending_lines,
            t0,
            t1,
        });
        self.c_off += member.len() as u64;
        self.u_off += pending.len() as u64;
        self.line_no += pending_lines;
        self.compressed.extend_from_slice(&member);
    }
}

fn write_encoded(
    path: &Path,
    sidecar: &Path,
    encoded: &[(Vec<u8>, Option<i64>)],
    timestamp_key: &str,
) -> Result<WriteReport, String> {
    let mut state = FlushState {
        members: Vec::new(),
        compressed: Vec::new(),
        c_hash: sha2::Sha256::new(),
        u_hash: sha2::Sha256::new(),
        c_off: 0,
        u_off: 0,
        line_no: 0,
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_lines = 0u64;
    let mut pending_t0: Option<i64> = None;
    let mut pending_t1: Option<i64> = None;
    for (line, timestamp) in encoded {
        pending.extend_from_slice(line);
        pending.push(b'\n');
        pending_lines += 1;
        if let Some(timestamp) = timestamp {
            pending_t0 = Some(pending_t0.map_or(*timestamp, |value| value.min(*timestamp)));
            pending_t1 = Some(pending_t1.map_or(*timestamp, |value| value.max(*timestamp)));
        }
        if pending.len() as u64 >= DEFAULT_TARGET_CHUNK_BYTES {
            state.flush(&pending, pending_lines, pending_t0, pending_t1);
            pending.clear();
            pending_lines = 0;
            pending_t0 = None;
            pending_t1 = None;
        }
    }
    if pending_lines > 0 || state.members.is_empty() {
        state.flush(&pending, pending_lines, pending_t0, pending_t1);
    }
    let index = ChunkIndex {
        record_count: state.line_no,
        c_size: state.c_off,
        u_size: state.u_off,
        c_sha256: state
            .c_hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        u_sha256: state
            .u_hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        timestamps_sorted: bisectable(&state.members),
        members: state.members,
        target_chunk_bytes: DEFAULT_TARGET_CHUNK_BYTES,
        timestamp_key: timestamp_key.to_string(),
        data_file: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let data_changed = replace_if_changed(path, sidecar, &state.compressed)?;
    let index_changed = crate::transcript_io::write_text_if_changed(sidecar, &index.to_text())?;
    Ok(WriteReport {
        path: path.to_path_buf(),
        index_path: sidecar.to_path_buf(),
        index,
        data_changed,
        index_changed,
    })
}

fn replace_if_changed(path: &Path, sidecar: &Path, bytes: &[u8]) -> Result<bool, String> {
    if path.is_file() {
        if let Ok(existing) = std::fs::read(path) {
            if existing == bytes {
                return Ok(false);
            }
        }
    }
    let _ = std::fs::remove_file(sidecar);
    crate::transcript_io::atomic_write(path, bytes)?;
    Ok(true)
}

/// The digest of a byte string.
pub fn sha256_text(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// Read all records from a plain or chunked JSONL file (the
/// exporter's own outputs), decoding container members when a
/// sidecar index is present.
pub fn read_records(path: &Path) -> Result<Vec<Value>, String> {
    let sidecar = index_path_for(path);
    if sidecar.is_file() {
        let index_text = std::fs::read_to_string(&sidecar)
            .map_err(|error| crate::json::os_error(&error, &sidecar))?;
        let mut lines = index_text.lines();
        let header = lines
            .next()
            .ok_or_else(|| format!("{}: empty index", sidecar.display()))?;
        let header = crate::json::parse_json_lenient(header)
            .map_err(|error| format!("{}: {error}", sidecar.display()))?;
        let _ = header;
        let compressed =
            std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
        let mut records = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let entry = crate::json::parse_json_lenient(line)
                .map_err(|error| format!("{}: {error}", sidecar.display()))?;
            let c_off = entry.get("c_off").and_then(Value::as_u64).unwrap_or(0) as usize;
            let c_len = entry.get("c_len").and_then(Value::as_u64).unwrap_or(0) as usize;
            let member = compressed
                .get(c_off..c_off + c_len)
                .ok_or_else(|| format!("{}: member out of range", path.display()))?;
            let mut decoded = Vec::new();
            {
                use std::io::Read as _;
                flate2::read::GzDecoder::new(member)
                    .read_to_end(&mut decoded)
                    .map_err(|error| error.to_string())?;
            }
            for record_line in decoded.split(|byte| *byte == b'\n') {
                if record_line.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(record_line);
                records.push(
                    crate::json::parse_json_lenient(&text)
                        .map_err(|error| format!("{}: {error}", path.display()))?,
                );
            }
        }
        return Ok(records);
    }
    if path.extension().is_some_and(|ext| ext == "gz") {
        let compressed =
            std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
        let mut decoded = Vec::new();
        {
            use std::io::Read as _;
            flate2::read::MultiGzDecoder::new(&compressed[..])
                .read_to_end(&mut decoded)
                .map_err(|error| error.to_string())?;
        }
        let mut records = Vec::new();
        for record_line in decoded.split(|byte| *byte == b'\n') {
            if record_line.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(record_line);
            records.push(
                crate::json::parse_json_lenient(&text)
                    .map_err(|error| format!("{}: {error}", path.display()))?,
            );
        }
        return Ok(records);
    }
    if !path.is_file() {
        return Ok(Vec::new());
    }
    crate::transcript_io::read_jsonl(path)
}
