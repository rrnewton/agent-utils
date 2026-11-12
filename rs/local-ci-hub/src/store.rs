//! Fan-out `Store` contract, cursor rules, and connectors.
//!
//! Several connectors are active at once; writes fan out and are acked
//! independently, while exactly one connector is the deployment's read
//! authority. The in-process trait mirrors `ci-hub-store-wire/v1` (see
//! `common/docs/local-ci-hub/ci-hub-store-wire-v1.md`): per-event acks,
//! byte-exact duplicate identity, strictly sequential paging by cursor,
//! and bounded, streaming, digest-verified object fetch. Refusals are
//! disjoint from validation results: none is ever a test red or green.

use crate::digest::sha256_of;
use crate::index::{Cursor, ShardPos};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Connector role in a deployment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Durable record; its ack set is the visibility boundary.
    Record,
    /// Publication route; acks never stand in for the record ack.
    Publish,
    /// Single-host local store.
    Local,
}

impl Role {
    /// Wire name.
    pub fn as_wire(self) -> &'static str {
        match self {
            Role::Record => "record",
            Role::Publish => "publish",
            Role::Local => "local",
        }
    }
}

/// Frozen wire refusal classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalClass {
    /// Connector unreachable, timed out, or connection lost.
    Unavailable,
    /// Credentials missing or rejected.
    Auth,
    /// Throttled; back off and retry.
    RateLimited,
    /// Malformed request or negotiated-bound violation.
    BadRequest,
    /// Unknown protocol major version.
    VersionUnsupported,
    /// Same event_id, different line bytes.
    ConflictingEventId,
    /// Cursor cannot be honoured or rebound.
    CursorInvalid,
    /// Append-only violation under a fast-forward head.
    StoreRewrite,
    /// No object for the requested digest.
    ObjectMissing,
    /// Declared length above max_bytes; refused unread.
    ObjectTooLarge,
    /// Streamed bytes fail length or SHA-256 check.
    ObjectDigestMismatch,
}

impl RefusalClass {
    /// Wire class string.
    pub fn as_wire(self) -> &'static str {
        match self {
            RefusalClass::Unavailable => "unavailable",
            RefusalClass::Auth => "auth",
            RefusalClass::RateLimited => "rate-limited",
            RefusalClass::BadRequest => "bad-request",
            RefusalClass::VersionUnsupported => "version-unsupported",
            RefusalClass::ConflictingEventId => "conflicting-event-id",
            RefusalClass::CursorInvalid => "cursor-invalid",
            RefusalClass::StoreRewrite => "store-rewrite",
            RefusalClass::ObjectMissing => "object-missing",
            RefusalClass::ObjectTooLarge => "object-too-large",
            RefusalClass::ObjectDigestMismatch => "object-digest-mismatch",
        }
    }
}

/// One store refusal with its frozen class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreRefusal {
    /// Frozen refusal class.
    pub class: RefusalClass,
    /// Human-readable detail.
    pub message: String,
}

impl StoreRefusal {
    /// Build a refusal.
    pub fn new(class: RefusalClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

impl fmt::Display for StoreRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class.as_wire(), self.message)
    }
}

impl std::error::Error for StoreRefusal {}

/// Result alias for store operations.
pub type Result<T> = std::result::Result<T, StoreRefusal>;

/// One event line in an append batch (exact bytes are the identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchLine {
    /// Event identity named by the line.
    pub event_id: String,
    /// Exact event line bytes as text (no trailing newline).
    pub line: String,
}

/// Per-event acknowledgement status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AckStatus {
    /// Newly stored on this connector.
    Stored,
    /// Already stored with identical bytes.
    Duplicate,
    /// Refused on this connector; stays spooled upstream.
    Refused {
        /// Frozen refusal class.
        class: RefusalClass,
        /// Human-readable detail.
        message: String,
    },
}

/// One event's acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventAck {
    /// The event this ack answers.
    pub event_id: String,
    /// What happened to it on this connector.
    pub status: AckStatus,
}

/// Why a cursor was reset during a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetReason {
    /// Head does not descend from the cursor head (squash or rewrite).
    HeadRewrite,
    /// A shard prefix no longer verifies under the new head.
    PrefixMismatch,
    /// A previously indexed shard is gone.
    ShardRemoved,
}

impl ResetReason {
    /// Wire reason string.
    pub fn as_wire(self) -> &'static str {
        match self {
            ResetReason::HeadRewrite => "head-rewrite",
            ResetReason::PrefixMismatch => "prefix-mismatch",
            ResetReason::ShardRemoved => "shard-removed",
        }
    }
}

/// One stored line with its shard position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardEvent {
    /// Shard path the line lives in.
    pub shard: String,
    /// Byte offset of the line's first byte in its shard.
    pub offset: u64,
    /// The stored line text (no trailing newline).
    pub line: String,
}

/// One `read_from` page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadPage {
    /// Events in shard path order.
    pub events: Vec<ShardEvent>,
    /// Cursor advanced past exactly the returned bytes.
    pub next_cursor: Cursor,
    /// Whether another page is available.
    pub more: bool,
    /// Set when the read rebound a stale cursor; never a silent skip.
    pub cursor_reset: Option<ResetReason>,
}

/// Read limits for one page: whichever bound hits first ends the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadLimits {
    /// Maximum events in the page.
    pub max_events: usize,
    /// Maximum returned line bytes in the page.
    pub max_bytes: u64,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_events: 1000,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Bounded, streaming, digest-verified object reader.
///
/// The declared length is known before the first byte. Callers read the
/// stream, then call [`FetchStream::verify`]; bytes from an unverified or
/// aborted stream must be discarded.
pub struct FetchStream {
    inner: Box<dyn Read>,
    declared_len: u64,
    expected_sha256: String,
    hasher: Sha256,
    read: u64,
}

impl FetchStream {
    /// Wrap a reader of exactly `declared_len` bytes for `sha256`.
    pub fn new(inner: Box<dyn Read>, declared_len: u64, sha256: impl Into<String>) -> Self {
        Self {
            inner,
            declared_len,
            expected_sha256: sha256.into(),
            hasher: Sha256::new(),
            read: 0,
        }
    }

    /// Length declared before the first byte.
    pub fn declared_len(&self) -> u64 {
        self.declared_len
    }

    /// Read the rest of the stream (at most the declared length plus one
    /// overrun-probe byte) and verify length and digest.
    pub fn verify(mut self) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = self.read(&mut chunk).map_err(|e| {
                StoreRefusal::new(
                    RefusalClass::Unavailable,
                    format!("object read failed: {e}"),
                )
            })?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&chunk[..n]);
            if self.read > self.declared_len {
                return Err(StoreRefusal::new(
                    RefusalClass::ObjectDigestMismatch,
                    "object overruns its declared length",
                ));
            }
        }
        if self.read != self.declared_len {
            return Err(StoreRefusal::new(
                RefusalClass::ObjectDigestMismatch,
                "object is shorter than its declared length",
            ));
        }
        let digest = format!("{:x}", self.hasher.finalize());
        if digest != self.expected_sha256 {
            return Err(StoreRefusal::new(
                RefusalClass::ObjectDigestMismatch,
                "object sha256 differs",
            ));
        }
        Ok(data)
    }
}

impl fmt::Debug for FetchStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchStream")
            .field("declared_len", &self.declared_len)
            .field("expected_sha256", &self.expected_sha256)
            .field("read", &self.read)
            .finish_non_exhaustive()
    }
}

impl Read for FetchStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

/// The store contract every connector implements.
pub trait Store {
    /// This connector's role.
    fn role(&self) -> Role;
    /// Stable connector id used as cursor authority.
    fn connector_id(&self) -> &str;
    /// Append one batch; acks are per event and independent.
    fn append(&mut self, batch: &[BatchLine]) -> Result<Vec<EventAck>>;
    /// Current head cursor.
    fn head(&self) -> Result<Cursor>;
    /// Read one page after `cursor`.
    fn read_from(&self, cursor: &Cursor, limits: ReadLimits) -> Result<ReadPage>;
    /// Fetch one object by digest, bounded by `max_bytes` (refused unread
    /// above the bound).
    fn fetch_object(&self, sha256: &str, max_bytes: u64) -> Result<FetchStream>;
}

/// Shared tail logic over whole shard byte strings.
///
/// `descends` says whether `current_head` descends from `cursor.head`
/// (fresh cursors with an empty head always descend trivially). Returns
/// the page plus reset bookkeeping per the contract's cursor rules.
fn tail_shards(
    authority: &str,
    current_head: &str,
    shards: &BTreeMap<String, Vec<u8>>,
    cursor: &Cursor,
    descends: bool,
    limits: ReadLimits,
) -> Result<ReadPage> {
    let fresh = cursor.head.is_empty();
    let mut reset: Option<ResetReason> = None;
    let mut starts: BTreeMap<String, u64> = BTreeMap::new();
    for (path, bytes) in shards {
        let Some(pos) = cursor.shards.get(path) else {
            starts.insert(path.clone(), 0);
            continue;
        };
        let offset = pos.offset as usize;
        let prefix_ok = offset <= bytes.len() && sha256_of(&bytes[..offset]) == pos.prefix_sha256;
        if prefix_ok {
            starts.insert(path.clone(), pos.offset);
        } else if descends && !fresh {
            return Err(StoreRefusal::new(
                RefusalClass::StoreRewrite,
                format!("shard {path} prefix changed under a fast-forward head"),
            ));
        } else {
            reset = Some(ResetReason::PrefixMismatch);
            starts.insert(path.clone(), 0);
        }
    }
    if !fresh && !descends && reset.is_none() {
        // Pure squash: every prefix verified; rebind with an explicit reset.
        reset = Some(ResetReason::HeadRewrite);
    }
    for path in cursor.shards.keys() {
        if !shards.contains_key(path) {
            if descends && !fresh {
                return Err(StoreRefusal::new(
                    RefusalClass::StoreRewrite,
                    format!("shard {path} removed under a fast-forward head"),
                ));
            }
            if reset.is_none() {
                reset = Some(ResetReason::ShardRemoved);
            }
        }
    }
    let mut events: Vec<ShardEvent> = Vec::new();
    let mut spent_bytes: u64 = 0;
    let mut more = false;
    let mut next_shards: BTreeMap<String, ShardPos> = BTreeMap::new();
    for (path, bytes) in shards {
        let mut position = starts.get(path).copied().unwrap_or(0) as usize;
        if !more {
            while position < bytes.len() {
                // Lines are LF-terminated; a trailing partial line is not
                // yet a stored event and stays unread.
                let Some(newline) = bytes[position..].iter().position(|b| *b == b'\n') else {
                    break;
                };
                let end = position + newline;
                let line_bytes = &bytes[position..end];
                let line = String::from_utf8_lossy(line_bytes).into_owned();
                let cost = line.len() as u64;
                if !events.is_empty()
                    && (events.len() >= limits.max_events || spent_bytes + cost > limits.max_bytes)
                {
                    more = true;
                    break;
                }
                if events.len() >= limits.max_events {
                    more = true;
                    break;
                }
                events.push(ShardEvent {
                    shard: path.clone(),
                    offset: position as u64,
                    line,
                });
                spent_bytes += cost;
                position = end + 1;
            }
            // A shard with unread complete lines left means more pages.
            if !more && position < bytes.len() && bytes[position..].contains(&b'\n') {
                // Only reachable when the byte budget stopped the page on
                // its first line of this shard after prior shards filled it.
                more = events.len() >= limits.max_events;
            }
        }
        next_shards.insert(
            path.clone(),
            ShardPos {
                offset: position as u64,
                prefix_sha256: sha256_of(&bytes[..position]),
            },
        );
    }
    Ok(ReadPage {
        events,
        next_cursor: Cursor {
            authority: authority.to_owned(),
            head: current_head.to_owned(),
            shards: next_shards,
        },
        more,
        cursor_reset: reset,
    })
}

/// In-memory connector: full contract semantics for tests and fan-out
/// composition. Head tokens are `mem-<n>` bumped on every mutation;
/// ancestry is token order, with [`MemoryStore::squash_to`] simulating a
/// non-descendant head.
pub struct MemoryStore {
    id: String,
    role: Role,
    head_seq: u64,
    squashed: bool,
    shards: BTreeMap<String, Vec<u8>>,
    by_id: BTreeMap<String, String>,
    objects: BTreeMap<String, Vec<u8>>,
}

impl MemoryStore {
    /// Empty store with one default shard.
    pub fn new(id: impl Into<String>, role: Role) -> Self {
        let mut shards = BTreeMap::new();
        shards.insert("ledger/local/local.jsonl".to_owned(), Vec::new());
        Self {
            id: id.into(),
            role,
            head_seq: 0,
            squashed: false,
            shards,
            by_id: BTreeMap::new(),
            objects: BTreeMap::new(),
        }
    }

    fn head_token(&self) -> String {
        format!("mem-{}", self.head_seq)
    }

    /// Simulate a squash: same shard bytes, non-descendant head token.
    pub fn squash(&mut self) {
        self.head_seq += 1000;
        self.squashed = true;
    }

    /// Store one evidence object under its digest.
    pub fn put_object(&mut self, data: Vec<u8>) -> String {
        let digest = sha256_of(&data);
        self.objects.insert(digest.clone(), data);
        digest
    }

    fn descends(&self, cursor: &Cursor) -> bool {
        if cursor.head.is_empty() || self.squashed {
            return cursor.head.is_empty();
        }
        match cursor
            .head
            .strip_prefix("mem-")
            .and_then(|n| n.parse::<u64>().ok())
        {
            Some(seq) => seq <= self.head_seq,
            None => false,
        }
    }
}

impl Store for MemoryStore {
    fn role(&self) -> Role {
        self.role
    }

    fn connector_id(&self) -> &str {
        &self.id
    }

    fn append(&mut self, batch: &[BatchLine]) -> Result<Vec<EventAck>> {
        let mut acks = Vec::with_capacity(batch.len());
        let shard = "ledger/local/local.jsonl".to_owned();
        for line in batch {
            match self.by_id.get(&line.event_id) {
                Some(existing) if *existing == line.line => {
                    acks.push(EventAck {
                        event_id: line.event_id.clone(),
                        status: AckStatus::Duplicate,
                    });
                }
                Some(_) => {
                    acks.push(EventAck {
                        event_id: line.event_id.clone(),
                        status: AckStatus::Refused {
                            class: RefusalClass::ConflictingEventId,
                            message: format!(
                                "event_id {} already stored with different bytes",
                                line.event_id
                            ),
                        },
                    });
                }
                None => {
                    self.by_id.insert(line.event_id.clone(), line.line.clone());
                    let bytes = self.shards.entry(shard.clone()).or_default();
                    bytes.extend_from_slice(line.line.as_bytes());
                    bytes.push(b'\n');
                    self.head_seq += 1;
                    self.squashed = false;
                    acks.push(EventAck {
                        event_id: line.event_id.clone(),
                        status: AckStatus::Stored,
                    });
                }
            }
        }
        Ok(acks)
    }

    fn head(&self) -> Result<Cursor> {
        let shards = self
            .shards
            .iter()
            .map(|(path, bytes)| {
                (
                    path.clone(),
                    ShardPos {
                        offset: bytes.len() as u64,
                        prefix_sha256: sha256_of(bytes),
                    },
                )
            })
            .collect();
        Ok(Cursor {
            authority: self.id.clone(),
            head: self.head_token(),
            shards,
        })
    }

    fn read_from(&self, cursor: &Cursor, limits: ReadLimits) -> Result<ReadPage> {
        tail_shards(
            &self.id,
            &self.head_token(),
            &self.shards,
            cursor,
            self.descends(cursor),
            limits,
        )
    }

    fn fetch_object(&self, sha256: &str, max_bytes: u64) -> Result<FetchStream> {
        let Some(data) = self.objects.get(sha256) else {
            return Err(StoreRefusal::new(
                RefusalClass::ObjectMissing,
                "no object for this digest",
            ));
        };
        if data.len() as u64 > max_bytes {
            return Err(StoreRefusal::new(
                RefusalClass::ObjectTooLarge,
                format!(
                    "declared length {} exceeds max_bytes {max_bytes}",
                    data.len()
                ),
            ));
        }
        Ok(FetchStream::new(
            Box::new(std::io::Cursor::new(data.clone())),
            data.len() as u64,
            sha256.to_owned(),
        ))
    }
}

/// Fan-out composition: writes go to every connector, reads come from the
/// one designated authority.
pub struct FanOut {
    stores: Vec<Box<dyn Store>>,
    authority: String,
}

impl FanOut {
    /// Compose connectors; `authority` must name one of their ids.
    pub fn new(stores: Vec<Box<dyn Store>>, authority: impl Into<String>) -> Self {
        Self {
            stores,
            authority: authority.into(),
        }
    }

    /// Append to every connector, returning each ack set by connector id.
    /// The record authority's ack set is the visibility boundary; other
    /// results never stand in for it.
    pub fn append_all(&mut self, batch: &[BatchLine]) -> Vec<(String, Result<Vec<EventAck>>)> {
        self.stores
            .iter_mut()
            .map(|store| (store.connector_id().to_owned(), store.append(batch)))
            .collect()
    }

    /// The read authority connector.
    pub fn authority_store(&self) -> Result<&dyn Store> {
        self.stores
            .iter()
            .find(|store| store.connector_id() == self.authority)
            .map(|store| store.as_ref() as &dyn Store)
            .ok_or_else(|| {
                StoreRefusal::new(
                    RefusalClass::Unavailable,
                    "read authority is not configured",
                )
            })
    }

    /// Head cursor of the read authority.
    pub fn head(&self) -> Result<Cursor> {
        self.authority_store()?.head()
    }

    /// Read one page from the read authority.
    pub fn read_from(&self, cursor: &Cursor, limits: ReadLimits) -> Result<ReadPage> {
        self.authority_store()?.read_from(cursor, limits)
    }

    /// Fetch one object from the read authority.
    pub fn fetch_object(&self, sha256: &str, max_bytes: u64) -> Result<FetchStream> {
        self.authority_store()?.fetch_object(sha256, max_bytes)
    }
}

/// Git least-common-denominator connector (read side).
///
/// Shards are read from git objects at one pinned ref, never from a working
/// tree. Appends refuse in this slice: git publication (spool, flock,
/// ancestry ack) is runner work, and a refused append keeps the spool
/// upstream instead of pretending durability.
pub struct GitStore {
    id: String,
    role: Role,
    repo: PathBuf,
    git_ref: String,
    shard_roots: Vec<String>,
    evidence_root: Option<PathBuf>,
}

impl GitStore {
    /// Read shards under `shard_roots` from `git_ref` in `repo`.
    pub fn new(
        id: impl Into<String>,
        role: Role,
        repo: PathBuf,
        git_ref: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            role,
            repo,
            git_ref: git_ref.into(),
            shard_roots: vec!["ledger".to_owned(), "ledger-recovery".to_owned()],
            evidence_root: None,
        }
    }

    /// Evidence archive root for [`Store::fetch_object`].
    pub fn with_evidence_root(mut self, root: PathBuf) -> Self {
        self.evidence_root = Some(root);
        self
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                StoreRefusal::new(RefusalClass::Unavailable, format!("git unavailable: {e}"))
            })?;
        if !output.status.success() {
            return Err(StoreRefusal::new(
                RefusalClass::Unavailable,
                format!(
                    "git {} failed: {}",
                    args.first().unwrap_or(&""),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        Ok(output.stdout)
    }

    fn head_commit(&self) -> Result<String> {
        let out = self.git(&["rev-parse", &format!("{}^{{commit}}", self.git_ref)])?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        if ancestor.is_empty() {
            return true;
        }
        Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn shard_paths(&self, head: &str) -> Result<Vec<String>> {
        let mut args: Vec<String> = vec![
            "ls-tree".into(),
            "-r".into(),
            "-z".into(),
            "--name-only".into(),
            head.to_owned(),
            "--".into(),
        ];
        args.extend(self.shard_roots.iter().cloned());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.git(&arg_refs)?;
        let mut paths: Vec<String> = out
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .filter(|path| is_shard_path(path))
            .collect();
        paths.sort();
        Ok(paths)
    }

    fn shard_bytes(&self, head: &str, paths: &[String]) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut shards = BTreeMap::new();
        for path in paths {
            let bytes = self.git(&["cat-file", "blob", &format!("{head}:{path}")])?;
            shards.insert(path.clone(), bytes);
        }
        Ok(shards)
    }
}

/// Whether `path` names a ledger shard (`YYYY-MM[.part-NNNN].jsonl` under
/// a team/host directory, or a recovery shard under a month directory).
pub fn is_shard_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some(file) = parts.last() else {
        return false;
    };
    let Some(stem) = file.strip_suffix(".jsonl") else {
        return false;
    };
    let is_month = |text: &str| {
        text.len() == 7
            && text.as_bytes()[4] == b'-'
            && text
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 4 || b.is_ascii_digit())
    };
    let month_like = match stem.split_once(".part-") {
        Some((month, part)) => {
            is_month(month) && part.len() == 4 && part.bytes().all(|b| b.is_ascii_digit())
        }
        None => is_month(stem),
    };
    if !month_like {
        // Recovery shards carry an opaque file name under a month directory.
        return parts.len() >= 5 && is_month(parts[parts.len() - 2]);
    }
    parts.len() >= 4
}

impl Store for GitStore {
    fn role(&self) -> Role {
        self.role
    }

    fn connector_id(&self) -> &str {
        &self.id
    }

    fn append(&mut self, batch: &[BatchLine]) -> Result<Vec<EventAck>> {
        Ok(batch
            .iter()
            .map(|line| EventAck {
                event_id: line.event_id.clone(),
                status: AckStatus::Refused {
                    class: RefusalClass::BadRequest,
                    message: "git connector is read-only in this slice; event stays spooled"
                        .to_owned(),
                },
            })
            .collect())
    }

    fn head(&self) -> Result<Cursor> {
        let head = self.head_commit()?;
        let paths = self.shard_paths(&head)?;
        let shards_bytes = self.shard_bytes(&head, &paths)?;
        let shards = shards_bytes
            .iter()
            .map(|(path, bytes)| {
                (
                    path.clone(),
                    ShardPos {
                        offset: bytes.len() as u64,
                        prefix_sha256: sha256_of(bytes),
                    },
                )
            })
            .collect();
        Ok(Cursor {
            authority: self.id.clone(),
            head,
            shards,
        })
    }

    fn read_from(&self, cursor: &Cursor, limits: ReadLimits) -> Result<ReadPage> {
        let head = self.head_commit()?;
        let paths = self.shard_paths(&head)?;
        let shards = self.shard_bytes(&head, &paths)?;
        let descends = self.is_ancestor(&cursor.head, &head);
        tail_shards(&self.id, &head, &shards, cursor, descends, limits)
    }

    fn fetch_object(&self, sha256: &str, max_bytes: u64) -> Result<FetchStream> {
        let Some(root) = &self.evidence_root else {
            return Err(StoreRefusal::new(
                RefusalClass::ObjectMissing,
                "no evidence root configured",
            ));
        };
        // Objects live at run-evidence/<host>/<sha256>.json.zst; the host is
        // part of the address, so search the host directories.
        let evidence_dir = root.join("run-evidence");
        let mut found: Option<PathBuf> = None;
        if let Ok(hosts) = std::fs::read_dir(&evidence_dir) {
            for host in hosts.flatten() {
                let candidate = host.path().join(format!("{sha256}.json.zst"));
                if candidate.is_file() {
                    found = Some(candidate);
                    break;
                }
            }
        }
        let Some(path) = found else {
            return Err(StoreRefusal::new(
                RefusalClass::ObjectMissing,
                "no object for this digest",
            ));
        };
        let compressed = std::fs::read(&path).map_err(|e| {
            StoreRefusal::new(
                RefusalClass::Unavailable,
                format!("evidence unreadable: {e}"),
            )
        })?;
        // The stored object is zstd-compressed with an unknown expanded
        // length; decompress bounded by max_bytes (+1 probe) up front so the
        // declared length is known before streaming, per contract. An
        // object above the bound refuses before any byte is delivered.
        let data = decompress_bounded(&compressed, max_bytes)?;
        if sha256_of(&data) != sha256 {
            return Err(StoreRefusal::new(
                RefusalClass::ObjectDigestMismatch,
                "evidence object sha256 differs",
            ));
        }
        let len = data.len() as u64;
        Ok(FetchStream::new(
            Box::new(std::io::Cursor::new(data)),
            len,
            sha256.to_owned(),
        ))
    }
}

fn decompress_bounded(compressed: &[u8], max_bytes: u64) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut child = Command::new("zstd")
        .args(["--quiet", "--decompress", "--stdout", "--memory=128MB"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            StoreRefusal::new(RefusalClass::Unavailable, format!("zstd unavailable: {e}"))
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(compressed);
    }
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut out = Vec::new();
    stdout
        .by_ref()
        .take(max_bytes + 1)
        .read_to_end(&mut out)
        .map_err(|e| {
            StoreRefusal::new(
                RefusalClass::Unavailable,
                format!("evidence unreadable: {e}"),
            )
        })?;
    if out.len() as u64 > max_bytes {
        let _ = child.kill();
        let _ = child.wait();
        return Err(StoreRefusal::new(
            RefusalClass::ObjectTooLarge,
            format!("object expands past max_bytes {max_bytes}"),
        ));
    }
    let status = child
        .wait()
        .map_err(|e| StoreRefusal::new(RefusalClass::Unavailable, format!("zstd failed: {e}")))?;
    if !status.success() {
        return Err(StoreRefusal::new(
            RefusalClass::ObjectDigestMismatch,
            "evidence is not a valid zstd object",
        ));
    }
    Ok(out)
}
