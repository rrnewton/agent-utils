//! The queue directory: its phases, message identifiers, enqueue with its duplicate check, and
//! the reads and moves every drainer shares.
//!
//! The format itself is described in the crate documentation.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::error::{io_error, Error, ErrorKind, Result};
use crate::fsio::{
    atomic_json, atomic_json_create, create_private_directory, lock_exclusive, open_private_lock,
    read_json, repair_private_directory, sync_directory, transition, validate_private_directory,
    HeldLock, Waiter,
};
use fs2::FileExt;

/// Longest message identifier, in bytes.
pub const MESSAGE_ID_MAX: usize = 255;
/// Pending items, never typed.
pub const INBOX: &str = "inbox";
/// Items past the at-most-once barrier whose outcome is not settled.
pub const INFLIGHT: &str = "inflight";
/// Items whose delivery was confirmed.
pub const PROCESSED: &str = "processed";
/// Quarantined items: malformed, or possibly typed and never to be replayed.
pub const FAILED: &str = "failed";
/// Pending items that only a drainer which honours tombstones and `not_after` may type.
pub const INBOX_V2: &str = "inbox.v2";
/// Tombstones: `cancel/<id>.json` asks every drainer not to type item `<id>`.
pub const CANCEL: &str = "cancel";
/// Items a drainer set aside untyped: cancelled by a tombstone, or expired.
pub const CANCELLED: &str = "cancelled";
/// The lock that serializes drains, explicit-identifier enqueues and withdrawals.
pub const DELIVERY_LOCK: &str = ".delivery.lock";
/// The lock that serializes changes to the target binding.
pub const BINDING_LOCK: &str = ".binding.lock";
/// The target binding, written by the first send.
pub const TARGET_BINDING: &str = "target.json";
/// Item field: Unix time in milliseconds after which no drainer types the item.
pub const NOT_AFTER: &str = "not_after";
/// Item field: sender-chosen key naming what the item is the latest word on.
pub const SUPERSEDE_KEY: &str = "supersede_key";
/// Item field: identifier of the newer item that superseded this one.
pub const SUPERSEDED_BY: &str = "superseded_by";
/// Longest supersede key, in bytes.
pub const SUPERSEDE_KEY_MAX: usize = 255;

/// Exact durable location of one caller-selected queue message identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    /// The message is known not to have crossed the injection barrier.
    Pending,
    /// The message crossed the barrier and its outcome remains unsettled.
    Inflight,
    /// The native working transition confirmed delivery.
    Processed,
    /// Delivery may have occurred and automatic replay is unsafe.
    Failed,
    /// Set aside untyped: cancelled by a tombstone, or expired.
    Cancelled,
}

/// The phase directories of one queue.
#[derive(Clone, Debug)]
pub struct QueueDirectories {
    /// `inbox/`.
    pub inbox: PathBuf,
    /// `inflight/`.
    pub inflight: PathBuf,
    /// `processed/`.
    pub processed: PathBuf,
    /// `failed/`.
    pub failed: PathBuf,
    /// `inbox.v2/`, created by the first item that needs it.
    pub inbox_v2: PathBuf,
    /// `cancel/`, created by the first tombstone.
    pub cancel: PathBuf,
    /// `cancelled/`, created by the first item set aside.
    pub cancelled: PathBuf,
}

impl QueueDirectories {
    /// The phase directories under the queue `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            inbox: root.join(INBOX),
            inflight: root.join(INFLIGHT),
            processed: root.join(PROCESSED),
            failed: root.join(FAILED),
            inbox_v2: root.join(INBOX_V2),
            cancel: root.join(CANCEL),
            cancelled: root.join(CANCELLED),
        }
    }

    /// The four phase directories every queue has, in lifecycle order. The lanes
    /// ([`Self::lanes`]) exist only once something needed them, so a queue that never used one
    /// stays readable by drainers that know only these four.
    #[must_use]
    pub fn all(&self) -> [&Path; 4] {
        [&self.inbox, &self.inflight, &self.processed, &self.failed]
    }

    /// The directories a drainer must understand before it may touch the queue at all:
    /// `inbox.v2/`, `cancel/` and `cancelled/`.
    #[must_use]
    pub fn lanes(&self) -> [&Path; 3] {
        [&self.inbox_v2, &self.cancel, &self.cancelled]
    }

    /// Every directory that can hold an item, with the state an item in it has.
    #[must_use]
    pub fn states(&self) -> [(&Path, MessageState); 6] {
        [
            (&self.inbox, MessageState::Pending),
            (&self.inbox_v2, MessageState::Pending),
            (&self.inflight, MessageState::Inflight),
            (&self.processed, MessageState::Processed),
            (&self.failed, MessageState::Failed),
            (&self.cancelled, MessageState::Cancelled),
        ]
    }

    /// Whether any lane directory exists.
    #[must_use]
    pub fn uses_lanes(&self) -> bool {
        self.lanes()
            .iter()
            .any(|lane| fs::symlink_metadata(lane).is_ok())
    }
}

/// Which pending directory an enqueue writes to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lane {
    /// `inbox/`: every drainer types it.
    Inbox,
    /// `inbox.v2/`: only drainers that honour tombstones and `not_after` see it.
    InboxV2,
}

/// Create the lane directory `path` of the queue `root` if it is absent.
pub fn prepare_lane(root: &Path, path: &Path) -> Result<()> {
    let existed = fs::symlink_metadata(path).is_ok();
    create_private_directory(path, "queue lane directory", false, true)?;
    if !existed {
        sync_directory(root)?;
    }
    Ok(())
}

/// Create the queue `root` and its phase directories, private to the caller, and sync them.
pub fn prepare(root: &Path) -> Result<QueueDirectories> {
    let root_existed = fs::symlink_metadata(root).is_ok();
    create_private_directory(root, "queue directory", true, true)?;
    if !root_existed {
        if let Some(parent) = root.parent().filter(|path| !path.as_os_str().is_empty()) {
            sync_directory(parent)?;
        }
    }
    let directories = QueueDirectories::new(root);
    for path in directories.all() {
        create_private_directory(path, "queue state directory", true, true)?;
    }
    // A lane is created only when needed, sometimes by another program; one made under a 022
    // umask would stop every read-only lookup, so a writer repairs it here.
    for path in directories.lanes() {
        if fs::symlink_metadata(path).is_ok() {
            repair_private_directory(path, "queue lane directory")?;
        }
    }
    sync_directory(root)?;
    Ok(directories)
}

/// Require an existing queue and its existing phase directories to be private; a queue or
/// phase that does not exist yet passes. Nothing is changed: a status read stays read-only, and
/// the error names the mode and the repair. Any send, drain, enqueue or cancel ([`prepare`])
/// repairs a directory only its owner can write to.
pub fn validate_existing_queue(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root) {
        Ok(_) => validate_private_directory(root, "queue directory", false)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("inspect queue directory", root, error)),
    }
    let directories = QueueDirectories::new(root);
    for directory in directories.all().into_iter().chain(directories.lanes()) {
        match fs::symlink_metadata(directory) {
            Ok(_) => validate_private_directory(directory, "queue state directory", false)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect queue state directory", directory, error)),
        }
    }
    Ok(())
}

/// The device and inode of a queue directory, so a lock holder can tell the directory was not
/// replaced while it waited.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

/// The identity of the queue directory `root`, which must be a private directory owned by the
/// caller.
pub fn directory_identity(root: &Path) -> Result<DirectoryIdentity> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| io_error("inspect queue directory", root, error))?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if !metadata.file_type().is_dir() || metadata.uid() != uid {
        return Err(Error::unsafe_path(format!(
            "unsafe queue directory: {}",
            root.display()
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error::unsafe_path(format!(
            "queue directory is not private: {}",
            root.display()
        )));
    }
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

/// After `lock` (opened at `path`) was taken, require the queue directory to be the one
/// identified before waiting, and the lock file to be the private file still at `path`.
pub fn verify_queue_lock(
    root: &Path,
    identity: DirectoryIdentity,
    path: &Path,
    lock: &File,
    purpose: &str,
) -> Result<()> {
    if directory_identity(root)? != identity {
        return Err(Error::unsafe_path(format!(
            "queue directory changed while waiting for {purpose}: {}",
            root.display()
        )));
    }
    let current = fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("inspect {purpose} after locking"), path, error))?;
    let opened = lock
        .metadata()
        .map_err(|error| io_error(&format!("inspect {purpose} after locking"), path, error))?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    for metadata in [&current, &opened] {
        if !metadata.file_type().is_file()
            || metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(Error::unsafe_path(format!(
                "unsafe {purpose}: {}",
                path.display()
            )));
        }
    }
    if (current.dev(), current.ino()) != (opened.dev(), opened.ino()) {
        return Err(Error::unsafe_path(format!(
            "{purpose} changed while waiting: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Take the queue's delivery lock, waiting through `waiter`, and verify it against `identity`.
/// The lock is held until the returned file is dropped.
pub fn lock_delivery(
    root: &Path,
    identity: DirectoryIdentity,
    waiter: &dyn Waiter,
) -> Result<File> {
    let lock_path = root.join(DELIVERY_LOCK);
    let lock = open_private_lock(&lock_path, "queue delivery lock")?;
    lock_exclusive(&lock, &lock_path, "queue delivery", waiter)?;
    verify_queue_lock(root, identity, &lock_path, &lock, "queue delivery lock")?;
    Ok(lock)
}

/// Require `identifier` to be a valid message identifier: 1-255 ASCII letters, digits, dots,
/// underscores or hyphens, starting with a letter or digit.
pub fn validate_message_id(identifier: &str) -> Result<()> {
    let bytes = identifier.as_bytes();
    let first_ok = bytes.first().is_some_and(u8::is_ascii_alphanumeric);
    let rest_ok = bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !first_ok || !rest_ok || bytes.len() > MESSAGE_ID_MAX {
        return Err(Error::invalid(
            "message id must be 1-255 ASCII letters, digits, dots, underscores, or hyphens and must start with a letter or digit",
        ));
    }
    Ok(())
}

/// Require `key` to be a valid supersede key: 1-255 bytes with no control characters.
/// Keys name a subject within one queue (`status-request:lane:hash`), so punctuation such as
/// colons is fine; they are compared byte for byte.
pub fn validate_supersede_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > SUPERSEDE_KEY_MAX
        || key.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
    {
        return Err(Error::invalid(
            "supersede key must be 1-255 bytes with no control characters",
        ));
    }
    Ok(())
}

/// A fresh message identifier: the wall-clock time in nanoseconds, twenty digits, then the
/// process identifier.
#[must_use]
pub fn generated_message_id() -> String {
    let nanoseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanoseconds:020}-{}", std::process::id())
}

/// The per-item claim lock of an inflight item (#1480): the drain that claimed
/// the item holds this file's flock while it waits or types without the queue
/// lock. The flock dies with its holder, so start-of-drain recovery can tell a
/// live claim (leave it for its owner — never lost, never typed twice) from a
/// dead drainer's claim (recover exactly as before), with no wall-clock wait.
pub fn claim_lock_path(directories: &QueueDirectories, filename: &std::ffi::OsStr) -> PathBuf {
    directories
        .inflight
        .join(format!("{}.claim", filename.to_string_lossy()))
}

/// Take the claim lock for an inflight item. `Ok(None)` when a live drain
/// already holds it.
pub fn take_claim_lock(
    directories: &QueueDirectories,
    filename: &std::ffi::OsStr,
) -> Result<Option<HeldLock>> {
    let path = claim_lock_path(directories, filename);
    let file = open_private_lock(&path, "delivery claim lock")?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(HeldLock::new(file))),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(io_error("lock delivery claim", &path, error)),
    }
}

/// Whether a live drain holds the claim lock of an inflight item.
#[must_use]
pub fn claim_owner_alive(directories: &QueueDirectories, filename: &std::ffi::OsStr) -> bool {
    let path = claim_lock_path(directories, filename);
    let Ok(file) = File::open(&path) else {
        return false;
    };
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        Err(_) => true,
    }
}

/// Remove an inflight item's claim lock file once the item has left `inflight/`.
pub fn remove_claim_lock(directories: &QueueDirectories, filename: &std::ffi::OsStr) {
    let _ = fs::remove_file(claim_lock_path(directories, filename));
}

/// Seconds since the Unix epoch, as the queue's timestamps record them.
#[must_use]
pub fn unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Persist one item in `inbox/` and return its identifier.
///
/// The queue is created if absent. `message_id` is used when given, else a generated one. With
/// `serialize`, the delivery lock is held across the duplicate check and the write, so the check
/// is exact against a drain moving items between phases; generated identifiers can skip it,
/// since the no-replace create below refuses a name that exists in the inbox. An identifier
/// already present in any phase is refused with [`ErrorKind::Duplicate`]. `document` builds the
/// item from its identifier once the check has passed; it must be an object with a nonempty
/// string `text` and, if it has an `id`, the same identifier, or nothing is written.
pub fn enqueue(
    root: &Path,
    message_id: Option<&str>,
    serialize: bool,
    waiter: &dyn Waiter,
    document: impl FnOnce(&str) -> Value,
) -> Result<String> {
    enqueue_into(root, Lane::Inbox, message_id, serialize, waiter, document)
}

/// What one enqueue did: the new item's identifier, and the identifiers of the
/// still-pending items its `supersede_key` set aside (empty without a key).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Enqueued {
    /// The new item's identifier.
    pub id: String,
    /// Identifiers set aside as superseded by this enqueue, oldest first.
    pub superseded: Vec<String>,
}

/// [`enqueue`] into `lane`. An item for [`Lane::InboxV2`] may also carry `not_after`, a Unix time
/// in milliseconds after which no drainer types it; that lane is created when first used.
pub fn enqueue_into(
    root: &Path,
    lane: Lane,
    message_id: Option<&str>,
    serialize: bool,
    waiter: &dyn Waiter,
    document: impl FnOnce(&str) -> Value,
) -> Result<String> {
    Ok(enqueue_into_outcome(root, lane, message_id, serialize, waiter, document)?.id)
}

/// [`enqueue_into`], also reporting which pending items were superseded.
///
/// When the new item carries a `supersede_key`, every still-pending item in the queue (`inbox/`
/// or `inbox.v2/`) with the same key is set aside untyped into `cancelled/` with
/// `cancel_reason: superseded` and `superseded_by` naming the new item, so at most one pending
/// item per key survives. Delivered history is never touched: items in `inflight/`,
/// `processed/`, `failed/` or `cancelled/` keep their state, items without a key are never
/// superseded, and an item already stranded in a composer (`stranded_at` or `paste_started_at`
/// recorded) is left for the drain paths that own composers. The new item keeps its own fresh
/// `queued_at`: it takes its place at the back of the queue, it does not inherit the replaced
/// item's position. A key requires `serialize`: the scan, the set-asides and the write must be
/// exact against a drain moving items between phases.
pub fn enqueue_into_outcome(
    root: &Path,
    lane: Lane,
    message_id: Option<&str>,
    serialize: bool,
    waiter: &dyn Waiter,
    document: impl FnOnce(&str) -> Value,
) -> Result<Enqueued> {
    let directories = prepare(root)?;
    let identifier = message_id.map_or_else(generated_message_id, str::to_owned);
    validate_message_id(&identifier)?;
    let filename = format!("{identifier}.json");
    let directory = match lane {
        Lane::Inbox => &directories.inbox,
        Lane::InboxV2 => {
            prepare_lane(root, &directories.inbox_v2)?;
            &directories.inbox_v2
        }
    };
    let path = directory.join(&filename);
    let _lock = if serialize {
        Some(lock_delivery(root, directory_identity(root)?, waiter)?)
    } else {
        None
    };
    if directories
        .states()
        .iter()
        .any(|(directory, _)| fs::symlink_metadata(directory.join(&filename)).is_ok())
    {
        return Err(duplicate(&identifier));
    }
    let document = document(&identifier);
    let lane_fields_ok = match lane {
        Lane::Inbox => document.get(NOT_AFTER).is_none_or(Value::is_null),
        Lane::InboxV2 => document
            .get(NOT_AFTER)
            .is_none_or(|value| value.is_null() || value.as_u64().is_some()),
    };
    if !lane_fields_ok {
        return Err(Error::invalid(format!(
            "queued message {identifier}: not_after must be a Unix time in milliseconds, and only an inbox.v2 item may carry one"
        )));
    }
    let supersede_key = match document.get(SUPERSEDE_KEY) {
        None | Some(Value::Null) => None,
        Some(value) => {
            let Some(key) = value.as_str() else {
                return Err(Error::invalid(format!(
                    "queued message {identifier}: supersede_key must be a string"
                )));
            };
            validate_supersede_key(key)
                .map_err(|error| Error::invalid(format!("queued message {identifier}: {error}")))?;
            if !serialize {
                return Err(Error::invalid(format!(
                    "queued message {identifier}: supersede_key requires a serialized enqueue"
                )));
            }
            Some(key.to_owned())
        }
    };
    if document
        .get("text")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || document
            .get("id")
            .is_some_and(|id| id.as_str() != Some(identifier.as_str()))
    {
        return Err(Error::invalid(format!(
            "queued message {identifier} must be an object with a nonempty string text field and its own id"
        )));
    }
    atomic_json_create(&path, &document).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            duplicate(&identifier)
        } else {
            io_error("persist queued message", &path, error)
        }
    })?;
    // The new item is durable before anything is set aside, so a failure here can duplicate a
    // notice but never lose one; the delivery lock (required above) keeps a drain from typing
    // either copy in between.
    let mut superseded = Vec::new();
    if let Some(key) = supersede_key {
        let mut matches: Vec<(Option<f64>, PathBuf, Map<String, Value>)> = Vec::new();
        for inbox in [&directories.inbox, &directories.inbox_v2] {
            if fs::symlink_metadata(inbox).is_err() {
                continue;
            }
            for candidate in json_paths(inbox)? {
                if candidate == path {
                    continue;
                }
                let Ok(raw) = read_json(&candidate) else {
                    continue;
                };
                if raw.get(SUPERSEDE_KEY).and_then(Value::as_str) != Some(key.as_str()) {
                    continue;
                }
                // An item a drain already pasted into a composer stays with the stale-draft
                // paths; setting it aside here would misattribute that draft.
                if raw.get("stranded_at").is_some_and(|v| !v.is_null())
                    || raw.get("paste_started_at").is_some_and(|v| !v.is_null())
                {
                    continue;
                }
                let Ok(mut old) = load_message(&candidate) else {
                    continue;
                };
                let queued_at = old.get("queued_at").and_then(Value::as_f64);
                old.insert(SUPERSEDED_BY.to_owned(), json!(identifier));
                matches.push((queued_at, candidate, old));
            }
        }
        matches.sort_by(|(a, _, _), (b, _, _)| match (a, b) {
            (Some(x), Some(y)) => x.total_cmp(y),
            _ => a.is_some().cmp(&b.is_some()),
        });
        for (_, candidate, old) in matches {
            let detail = format!("superseded by {identifier}");
            superseded.push(set_aside(
                root,
                &directories,
                &candidate,
                old,
                "superseded",
                &detail,
            )?);
        }
    }
    Ok(Enqueued {
        id: identifier,
        superseded,
    })
}

fn duplicate(identifier: &str) -> Error {
    Error::new(
        ErrorKind::Duplicate,
        format!("message id already exists: {identifier}"),
    )
}

/// The document of the most common item: `id`, `text`, `queued_at` and `delivery_attempts`.
#[must_use]
pub fn plain_item(identifier: &str, text: &str) -> Value {
    serde_json::to_value(crate::schema::QueueItem::plain(identifier, text))
        .expect("plain queue item serializes")
}

/// Locate one message identifier without scanning a queue directory.
///
/// `None` proves that no artifact with this identifier exists in any queue phase, as far as a
/// caller that excludes concurrent writers can tell. An item whose stored `id` differs from its
/// file name, or one present in two phases, is an error.
pub fn message_state(root: &Path, message_id: &str) -> Result<Option<MessageState>> {
    validate_message_id(message_id)?;
    if fs::symlink_metadata(root).is_err() {
        return Ok(None);
    }
    validate_existing_queue(root)?;
    let filename = format!("{message_id}.json");
    let directories = QueueDirectories::new(root);
    let mut observed = None;
    for (directory, state) in directories.states() {
        let path = directory.join(&filename);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let document = load_message(&path)?;
                if document.get("id").and_then(Value::as_str) != Some(message_id) {
                    return Err(Error::invalid(format!(
                        "queue artifact {} contains a different message id",
                        path.display()
                    )));
                }
                if observed.replace(state).is_some() {
                    return Err(Error::invalid(format!(
                        "message id {message_id:?} exists in multiple queue phases"
                    )));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect queue message", &path, error)),
        }
    }
    Ok(observed)
}

/// The identifiers waiting in the queue's inbox and `inbox.v2/`, sorted by name; empty when the
/// queue does not exist.
pub fn pending_identifiers(root: &Path) -> Result<Vec<String>> {
    if fs::symlink_metadata(root).is_err() {
        return Ok(Vec::new());
    }
    validate_existing_queue(root)?;
    let directories = QueueDirectories::new(root);
    let mut identifiers = identifiers_if_directory(&directories.inbox)?;
    identifiers.extend(identifiers_if_directory(&directories.inbox_v2)?);
    identifiers.sort();
    Ok(identifiers)
}

/// Take the item `message_id` back out of the inbox, untyped, and say whether it was there.
///
/// The delivery lock is held, so an item is either withdrawn or typed, never both. An item in
/// any other phase is left alone. The file is removed, so the identifier can be queued again.
pub fn withdraw(root: &Path, message_id: &str, waiter: &dyn Waiter) -> Result<bool> {
    validate_message_id(message_id)?;
    if fs::symlink_metadata(root).is_err() {
        return Ok(false);
    }
    validate_existing_queue(root)?;
    let _lock = lock_delivery(root, directory_identity(root)?, waiter)?;
    let directories = QueueDirectories::new(root);
    let mut found = None;
    for inbox in [&directories.inbox, &directories.inbox_v2] {
        let path = inbox.join(format!("{message_id}.json"));
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                found = Some((inbox.clone(), path));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect queue message", &path, error)),
        }
    }
    let Some((inbox, path)) = found else {
        return Ok(false);
    };
    let document = load_message(&path)?;
    if document.get("id").and_then(Value::as_str) != Some(message_id) {
        return Err(Error::invalid(format!(
            "queue artifact {} contains a different message id",
            path.display()
        )));
    }
    fs::remove_file(&path).map_err(|error| io_error("withdraw queued message", &path, error))?;
    sync_directory(&inbox)?;
    Ok(true)
}

/// Read one item: a JSON object with a nonempty string `text` and, if present, a string `id`.
pub fn load_message(path: &Path) -> Result<Map<String, Value>> {
    let value = read_json(path).map_err(|error| {
        Error::invalid(format!(
            "cannot read queued message {}: {error}",
            path.display()
        ))
    })?;
    let document = value.as_object().cloned().ok_or_else(|| {
        Error::invalid(format!(
            "queued message {} has no string text field",
            path.display()
        ))
    })?;
    if document
        .get("text")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(Error::invalid(format!(
            "queued message {} must have a nonempty string text field",
            path.display()
        )));
    }
    if document.get("id").is_some_and(|value| !value.is_string()) {
        return Err(Error::invalid(format!(
            "queued message {} has a non-string id field",
            path.display()
        )));
    }
    Ok(document)
}

/// An item's recorded delivery attempts: `delivery_attempts`, else `tui_delivery_attempts`,
/// else zero.
pub fn delivery_attempts(document: &Map<String, Value>, path: &Path) -> Result<u64> {
    let (key, value) = if let Some(value) = document.get("delivery_attempts") {
        ("delivery_attempts", Some(value))
    } else {
        (
            "tui_delivery_attempts",
            document.get("tui_delivery_attempts"),
        )
    };
    match value {
        None => Ok(0),
        Some(value) => value.as_u64().ok_or_else(|| {
            Error::invalid(format!(
                "queued message {} has an invalid nonnegative integer {key} field",
                path.display()
            ))
        }),
    }
}

/// Write the quarantine sidecar `<file>.error` beside `failed_path`.
pub fn failed_metadata(failed_path: &Path, outcome: &str, error: &str) -> Result<()> {
    let filename = failed_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::invalid("failed artifact name is not valid UTF-8"))?;
    atomic_json(
        &failed_path.with_file_name(format!("{filename}.error")),
        &serde_json::to_value(crate::schema::QuarantineSidecar {
            artifact: filename.to_owned(),
            outcome: if outcome == "invalid_message" {
                crate::schema::QuarantineOutcome::InvalidMessage
            } else {
                crate::schema::QuarantineOutcome::PossiblySubmitted
            },
            error: error.to_owned(),
            failed_at: unix_seconds(),
        })
        .expect("quarantine sidecar serializes"),
    )
}

/// Move the file at `path` into `failed` as it is, with a sidecar recording `outcome` and
/// `error`, and return its identifier.
pub fn quarantine_raw(path: &Path, failed: &Path, outcome: &str, error: &str) -> Result<String> {
    let filename = path.file_name().ok_or_else(|| {
        Error::invalid(format!(
            "queued message has no filename: {}",
            path.display()
        ))
    })?;
    let destination = failed.join(filename);
    transition(path, &destination)?;
    failed_metadata(&destination, outcome, error)?;
    message_id_from_path(path)
}

/// Return an inflight item that provably never reached the terminal to the
/// pending lane it came from, and say whether that happened. The proof is
/// two stamps a tracking drainer leaves: `paste_tracking: true` plus the
/// origin lane (`inflight_from`), written with the at-most-once barrier,
/// and `paste_started_at`, written immediately before its first terminal
/// input. Tracked with no paste started means input provably never began,
/// so the item may be typed by a later drain; anything else (an untracked
/// item from an older drainer, a paste that started, an unreadable item)
/// stays quarantined. The false-positive window this closes - a drainer
/// that died between the barrier and its first input - stranded live
/// trial messages as `possibly_submitted` although nothing was ever typed.
fn repend_never_typed(directories: &QueueDirectories, path: &Path) -> Result<bool> {
    // A cancelled item is never written back to a pending lane (#1775 (c)).
    if path.file_name().is_some_and(|filename| {
        directories
            .cancel
            .join(filename.to_string_lossy().into_owned())
            .exists()
    }) {
        return Ok(false);
    }
    let Ok(mut document) = load_message(path) else {
        return Ok(false);
    };
    // A never-typed item (no delivery attempt recorded) whose error is a
    // pre-typing refusal is also provably untyped, even if a stale paste
    // stamp survived: it must be retried, never "possibly submitted".
    let never_typed_error = document
        .get("delivery_attempts")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        == 0
        && document
            .get("delivery_error")
            .and_then(Value::as_str)
            .is_some_and(|e| {
                e.contains("nothing was typed") || e.contains("does not show a recognisable")
            });
    if !never_typed_error
        && (document.get("paste_tracking") != Some(&Value::Bool(true))
            || document.contains_key("paste_started_at"))
    {
        return Ok(false);
    }
    if never_typed_error {
        document.remove("paste_started_at");
    }
    let lane = match document.get("inflight_from").and_then(Value::as_str) {
        Some("inbox") => &directories.inbox,
        Some("inbox.v2") => &directories.inbox_v2,
        _ => return Ok(false),
    };
    if !lane.is_dir() {
        return Ok(false);
    }
    for field in [
        "possibly_submitted",
        "inflight_at",
        "paste_tracking",
        "inflight_from",
        "delivery_claimed_at",
    ] {
        document.remove(field);
    }
    document.insert("delivery_state".to_owned(), json!("pending"));
    atomic_json(path, &Value::Object(document))?;
    let filename = path.file_name().expect("listed path has filename");
    transition(path, &lane.join(filename))?;
    remove_claim_lock(directories, filename);
    Ok(true)
}

/// Return every inflight item a tracking drainer may have typed (its paste
/// started) to the pending lane it came from, marked stranded, and return
/// their identifiers (#884 item 4 (D)).
///
/// Such an item's paste may still sit in the agent's composer — a service
/// that stopped right after pasting, or crashed, leaves it there. Filing it
/// as possibly submitted strands that paste in the composer and abandons a
/// prompt the agent never saw. As a stranded pending item the next drain
/// presses Enter on the paste if the composer still holds it, and never
/// types the prompt again; if the paste is gone, the ordinary stranded
/// bounds report it possibly submitted. Untracked items (older drainers)
/// are left for [`recover_inflight`] to quarantine.
pub fn repend_stranded_inflight(directories: &QueueDirectories) -> Result<Vec<String>> {
    let mut repended = Vec::new();
    for path in json_paths(&directories.inflight)? {
        let Ok(mut document) = load_message(&path) else {
            continue;
        };
        // A cancelled item is never written back to a pending lane (#1775 (c)).
        if path.file_name().is_some_and(|filename| {
            directories
                .cancel
                .join(filename.to_string_lossy().into_owned())
                .exists()
        }) {
            continue;
        }
        // A live claim by a concurrent drain (#1480): leave it for its owner.
        if path
            .file_name()
            .is_some_and(|filename| claim_owner_alive(directories, filename))
        {
            continue;
        }
        if document.get("paste_tracking") != Some(&Value::Bool(true))
            || !document.contains_key("paste_started_at")
        {
            continue;
        }
        let lane = match document.get("inflight_from").and_then(Value::as_str) {
            Some("inbox") => &directories.inbox,
            Some("inbox.v2") => &directories.inbox_v2,
            _ => continue,
        };
        if !lane.is_dir() {
            continue;
        }
        if !document.contains_key("stranded_at") {
            let stranded_at = document
                .get("paste_started_at")
                .cloned()
                .unwrap_or_else(|| json!(unix_seconds()));
            document.insert("stranded_at".to_owned(), stranded_at);
        }
        for field in [
            "possibly_submitted",
            "inflight_at",
            "paste_tracking",
            "inflight_from",
            "paste_started_at",
            "delivery_claimed_at",
        ] {
            document.remove(field);
        }
        document.insert("delivery_state".to_owned(), json!("pending"));
        atomic_json(&path, &Value::Object(document))?;
        let filename = path.file_name().expect("listed path has filename");
        let identifier = message_id_from_path(&path)?;
        transition(&path, &lane.join(filename))?;
        remove_claim_lock(directories, filename);
        repended.push(identifier);
    }
    Ok(repended)
}

/// Quarantine every item left in `inflight/` as `possibly_submitted`, never replaying it, and
/// return their identifiers. An item a tracking drainer provably never
/// typed is returned to its pending lane instead ([`repend_never_typed`])
/// and is not returned. Only the holder of the delivery lock may call this.
pub fn recover_inflight(directories: &QueueDirectories) -> Result<Vec<String>> {
    let mut recovered = Vec::new();
    for path in json_paths(&directories.inflight)? {
        // A live claim by a concurrent drain (#1480): leave it for its owner.
        if path
            .file_name()
            .is_some_and(|filename| claim_owner_alive(directories, filename))
        {
            continue;
        }
        // A cancelled item is set aside, never quarantined or written back
        // to a pending lane (#1775 (c)).
        if let Ok(document) = load_message(&path) {
            if let Some((reason, detail)) = set_aside_reason(
                directories,
                &message_id_from_path(&path).unwrap_or_default(),
                &document,
                (unix_seconds() * 1000.0) as u64,
            ) {
                if reason == "cancelled" {
                    let root = directories
                        .inbox
                        .parent()
                        .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
                    set_aside(&root, directories, &path, document, reason, &detail)?;
                    continue;
                }
            }
        }
        if repend_never_typed(directories, &path)? {
            continue;
        }
        let filename = path.file_name().expect("listed path has filename");
        let destination = directories.failed.join(filename);
        transition(&path, &destination)?;
        remove_claim_lock(directories, filename);
        failed_metadata(
            &destination,
            "possibly_submitted",
            "recovered an inflight prompt after process restart; refusing automatic resubmission",
        )?;
        recovered.push(message_id_from_path(&path)?);
    }
    Ok(recovered)
}

/// The `*.json` files in `directory`, sorted by name.
pub fn json_paths(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(directory)
        .map_err(|error| io_error("list durable queue directory", directory, error))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| io_error("read durable queue directory entry", directory, error))?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(".json"))
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

/// The inbox's items in the order they were queued: by `queued_at`, then by file name.
///
/// A file named for a hash says nothing about arrival, so name order alone is not queue order.
/// An entry without a numeric `queued_at` comes first, by name: one written in a shape that
/// numbers its file names in sequence and records `queued_at` as text, if at all, or one that
/// cannot be read, which a drain then quarantines.
pub fn inbox_in_queue_order(inbox: &Path) -> Result<Vec<PathBuf>> {
    in_queue_order(&[inbox])
}

/// Every pending item of the queue, `inbox/` and `inbox.v2/` merged, in the order of
/// [`inbox_in_queue_order`]; for one `queued_at` and file name, the `inbox/` item first.
pub fn pending_in_queue_order(directories: &QueueDirectories) -> Result<Vec<PathBuf>> {
    if fs::symlink_metadata(&directories.inbox_v2).is_ok() {
        // A second inbox another writer created with a wider mode is made private here, as
        // `prepare` does for the four phases.
        validate_private_directory(&directories.inbox_v2, "queue lane directory", true)?;
        in_queue_order(&[&directories.inbox, &directories.inbox_v2])
    } else {
        in_queue_order(&[&directories.inbox])
    }
}

fn in_queue_order(directories: &[&Path]) -> Result<Vec<PathBuf>> {
    let mut entries: Vec<(Option<f64>, std::ffi::OsString, usize, PathBuf)> = Vec::new();
    for (lane, directory) in directories.iter().enumerate() {
        for path in json_paths(directory)? {
            let queued_at = read_json(&path)
                .ok()
                .and_then(|document| document.get("queued_at").and_then(Value::as_f64))
                // `total_cmp` puts -0.0 before 0.0; make them one time.
                .map(|seconds| if seconds == 0.0 { 0.0 } else { seconds });
            let name = path.file_name().unwrap_or_default().to_owned();
            entries.push((queued_at, name, lane, path));
        }
    }
    entries.sort_by(
        |(left_at, left, left_lane, _), (right_at, right, right_lane, _)| {
            match (left_at, right_at) {
                (Some(left_time), Some(right_time)) => left_time.total_cmp(right_time),
                _ => left_at.is_some().cmp(&right_at.is_some()),
            }
            .then_with(|| left.cmp(right))
            .then_with(|| left_lane.cmp(right_lane))
        },
    );
    Ok(entries.into_iter().map(|(_, _, _, path)| path).collect())
}

/// The current time as a Unix time in milliseconds, the unit of `not_after`.
#[must_use]
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// What [`cancel`] found.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelOutcome {
    /// The item was waiting and is now in `cancelled/`: no drainer, of any version, can type it.
    SetAside,
    /// The item waits untyped while a drain holds the queue; that drain, if it honours
    /// tombstones, sets it aside instead of typing it. A drainer that knows only the four phases
    /// does not read tombstones.
    Pending,
    /// No item has the identifier yet; the tombstone stays, so one that arrives later is set
    /// aside too.
    Unknown,
    /// A drain has taken the item and may already be typing it: the tombstone stops it only if
    /// the drain is still before its last check.
    Inflight,
    /// Too late: the item was typed, or quarantined after typing may have begun.
    TooLate,
    /// The item was already set aside.
    AlreadyCancelled,
}

/// A [`Waiter`] that never waits: a contended lock is given up at once.
struct NoWait;

impl Waiter for NoWait {
    fn cancelled(&self) -> bool {
        true
    }
    fn sleep(&self, _duration: std::time::Duration) {}
}

/// [`message_state`], retried a few times when a drain moving the item between directories makes
/// one look see it twice or lose it between listing and reading.
fn settled_state(root: &Path, message_id: &str) -> Result<Option<MessageState>> {
    let mut attempt = 0;
    loop {
        match message_state(root, message_id) {
            Ok(state) => return Ok(state),
            Err(error) if attempt < 5 && error.kind() == ErrorKind::Invalid => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Ask every drainer not to type item `message_id`, never waiting for a lock: write the tombstone
/// `cancel/<id>.json` (`id`, `reason`, `cancelled_at`), then, if no drain holds the queue, set a
/// waiting item aside at once ([`CancelOutcome::SetAside`]). Otherwise report where the item is:
/// a drain checks for the tombstone after it moves an item to `inflight/` and before it types, so
/// an item reported [`CancelOutcome::Pending`] is never typed by a drainer that honours
/// tombstones.
pub fn cancel(root: &Path, message_id: &str, reason: &str) -> Result<CancelOutcome> {
    validate_message_id(message_id)?;
    let directories = prepare(root)?;
    let filename = format!("{message_id}.json");
    let present = |directories: &QueueDirectories| -> Vec<MessageState> {
        directories
            .states()
            .iter()
            .filter(|(directory, _)| fs::symlink_metadata(directory.join(&filename)).is_ok())
            .map(|(_, state)| *state)
            .collect()
    };
    match settled_state(root, message_id) {
        Ok(Some(MessageState::Cancelled)) => return Ok(CancelOutcome::AlreadyCancelled),
        Ok(Some(MessageState::Processed | MessageState::Failed)) => {
            return Ok(CancelOutcome::TooLate);
        }
        Ok(_) => {}
        Err(_) if present(&directories).len() > 1 => {}
        Err(error) => return Err(error),
    }
    prepare_lane(root, &directories.cancel)?;
    let path = directories.cancel.join(&filename);
    let tombstone = serde_json::to_value(crate::schema::Tombstone {
        id: message_id.to_owned(),
        reason: reason.to_owned(),
        cancelled_at: unix_seconds(),
    })
    .expect("queue tombstone serializes");
    match atomic_json_create(&path, &tombstone) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error("write queue tombstone", &path, error)),
    }
    // With the delivery lock free, no drain is between its last check and its paste, so the item
    // can be set aside now, out of reach of every drainer.
    let had_cancelled = present(&directories).contains(&MessageState::Cancelled);
    let mut set_aside_pending = false;
    if let Ok(_lock) = lock_delivery(root, directory_identity(root)?, &NoWait) {
        for inbox in [&directories.inbox, &directories.inbox_v2] {
            let item = inbox.join(&filename);
            if fs::symlink_metadata(&item).is_ok() {
                let document = load_message(&item)?;
                set_aside(root, &directories, &item, document, "cancelled", reason)?;
                set_aside_pending = true;
            }
        }
        if set_aside_pending && !had_cancelled && present(&directories).len() <= 1 {
            return Ok(CancelOutcome::SetAside);
        }
    }
    // An id in several phases (#2022 cancel-resurrection): the terminal
    // record wins, a pending copy was retired above (or the tombstone
    // retires it at the drain holding the queue), and the ambiguity is
    // resolved instead of refused.
    let states = present(&directories);
    if states.len() > 1 {
        return Ok(if had_cancelled {
            CancelOutcome::AlreadyCancelled
        } else if set_aside_pending {
            CancelOutcome::SetAside
        } else if states.contains(&MessageState::Pending) {
            CancelOutcome::Pending
        } else {
            CancelOutcome::TooLate
        });
    }
    Ok(match settled_state(root, message_id)? {
        Some(MessageState::Pending) => CancelOutcome::Pending,
        Some(MessageState::Inflight) => CancelOutcome::Inflight,
        Some(MessageState::Processed | MessageState::Failed) => CancelOutcome::TooLate,
        Some(MessageState::Cancelled) => CancelOutcome::AlreadyCancelled,
        None => CancelOutcome::Unknown,
    })
}

/// Why the item `message_id`, whose document is `document`, must not be typed at `now_millis`:
/// `("cancelled", reason)` when a tombstone names it or a copy already sits in `cancelled/`
/// (a prompt retired by hand, with no tombstone written: #2022
/// cancel-resurrection), `("expired", detail)` when its `not_after` has passed, or
/// `None`. Drainers ask this just before typing, and every writer of an inbox entry asks it
/// before writing one back, so a retired prompt is never resurrected as pending.
pub fn set_aside_reason(
    directories: &QueueDirectories,
    message_id: &str,
    document: &Map<String, Value>,
    now_millis: u64,
) -> Option<(&'static str, String)> {
    if fs::symlink_metadata(directories.cancelled.join(format!("{message_id}.json"))).is_ok() {
        return Some(("cancelled", "already retired".to_owned()));
    }
    let tombstone = directories.cancel.join(format!("{message_id}.json"));
    if fs::symlink_metadata(&tombstone).is_ok() {
        let reason = read_json(&tombstone)
            .ok()
            .and_then(|value| {
                value
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        return Some(("cancelled", reason));
    }
    // A `null` not_after is no deadline, as an absent one is.
    let not_after = document.get(NOT_AFTER).filter(|value| !value.is_null())?;
    match not_after.as_u64() {
        Some(deadline) if deadline > now_millis => None,
        Some(deadline) => Some((
            "expired",
            format!("not_after {deadline} passed at {now_millis}"),
        )),
        None => Some(("expired", format!("invalid not_after {not_after}"))),
    }
}

/// Move the untyped item at `path` to `cancelled/` with `delivery_state: cancelled`,
/// `cancel_reason`, `cancel_detail` and `cancelled_at` added, and return its identifier.
pub fn set_aside(
    root: &Path,
    directories: &QueueDirectories,
    path: &Path,
    mut document: Map<String, Value>,
    reason: &str,
    detail: &str,
) -> Result<String> {
    prepare_lane(root, &directories.cancelled)?;
    let filename = path.file_name().ok_or_else(|| {
        Error::invalid(format!(
            "queued message has no filename: {}",
            path.display()
        ))
    })?;
    document.remove("possibly_submitted");
    document.remove("inflight_at");
    document.remove("paste_tracking");
    document.remove("inflight_from");
    document.remove("paste_started_at");
    document.remove("delivery_claimed_at");
    document.insert("delivery_state".to_owned(), json!("cancelled"));
    document.insert("cancel_reason".to_owned(), json!(reason));
    document.insert("cancel_detail".to_owned(), json!(detail));
    document.insert("cancelled_at".to_owned(), json!(unix_seconds()));
    atomic_json(path, &Value::Object(document))?;
    let destination = directories.cancelled.join(filename);
    if fs::symlink_metadata(&destination).is_ok() {
        // A copy of this item already sits in cancelled/ (it was retired,
        // and this pending copy is a resurrection or a leftover duplicate):
        // the cancelled copy is authoritative, so this copy is removed
        // rather than replacing it (#2022 cancel-resurrection).
        fs::remove_file(path)
            .map_err(|error| io_error("remove resurrected queue item", path, error))?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
    } else {
        transition(path, &destination)?;
    }
    remove_claim_lock(directories, filename);
    let identifier = message_id_from_path(path)?;
    // The tombstone has done its work: the item it named can never be typed now.
    let tombstone = directories.cancel.join(format!("{identifier}.json"));
    if reason == "cancelled" && fs::remove_file(&tombstone).is_ok() {
        sync_directory(&directories.cancel)?;
    }
    Ok(identifier)
}

/// The identifiers of the items in `directory`, sorted by name.
pub fn identifiers(directory: &Path) -> Result<Vec<String>> {
    json_paths(directory)?
        .iter()
        .map(|path| message_id_from_path(path))
        .collect()
}

/// [`identifiers`], or none when `directory` does not exist.
pub fn identifiers_if_directory(directory: &Path) -> Result<Vec<String>> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_dir() => identifiers(directory),
        Ok(_) => Err(Error::unsafe_path(format!(
            "unsafe queue state directory: {}",
            directory.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(io_error("inspect queue state directory", directory, error)),
    }
}

/// Format Unix seconds as an America/New_York wall time
/// (`YYYY-MM-DD HH:MM ET`), with the US DST rule (second Sunday of
/// March to first Sunday of November). Used only for the human header
/// on a re-targeted item, never for ordering.
fn format_eastern(unix_secs: f64) -> String {
    let secs = unix_secs as i64;
    let days = secs.div_euclid(86_400);
    // Day of week: 1970-01-01 was a Thursday (4 with Sunday = 0).
    let dow = (days + 4).rem_euclid(7);
    let (year, _month, _day) = civil_from_days(days);
    let nth_sunday = |y: i64, m: u32, n: i64| -> i64 {
        let first = days_from_civil(y, m, 1);
        let first_dow = (first + 4).rem_euclid(7);
        first + (7 - first_dow) % 7 + (n - 1) * 7
    };
    let dst = days >= nth_sunday(year, 3, 2) && days < nth_sunday(year, 11, 1);
    let _ = dow;
    let offset: i64 = if dst { -4 * 3600 } else { -5 * 3600 };
    let local = secs + offset;
    let ldays = local.div_euclid(86_400);
    let lsecs = local.rem_euclid(86_400);
    let (ly, lm, ld) = civil_from_days(ldays);
    format!(
        "{ly:04}-{lm:02}-{ld:02} {:02}:{:02} ET",
        lsecs / 3600,
        (lsecs % 3600) / 60
    )
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// One undelivered item set aside when its queue was archived (#2028 v2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrandedItem {
    /// Message identifier.
    pub id: String,
    /// Sender as recorded on the item, when known.
    pub sender: String,
    /// First line of the item text.
    pub first_line: String,
    /// First queued time in ET.
    pub queued_et: String,
    /// `stranded-archived`, `stranded-expired` or `possibly_submitted`.
    pub outcome: String,
}

/// Outcome of one archive set-aside sweep (#2028 v2).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArchiveSetAsideReport {
    /// Every item set aside or quarantined, oldest first.
    pub items: Vec<StrandedItem>,
}

fn stranded_item(document: &Map<String, Value>, id: &str, outcome: &str) -> StrandedItem {
    let sender = document
        .get("sender")
        .and_then(Value::as_str)
        .or_else(|| {
            document
                .get("envelope")
                .and_then(|e| e.get("from"))
                .and_then(|f| f.get("team"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
        .to_string();
    let text = document.get("text").and_then(Value::as_str).unwrap_or("");
    let first_line = text.lines().next().unwrap_or("").chars().take(80).collect();
    let queued_at = document
        .get("queued_at")
        .and_then(Value::as_f64)
        .unwrap_or_else(unix_seconds);
    StrandedItem {
        id: id.to_string(),
        sender,
        first_line,
        queued_et: format_eastern(queued_at),
        outcome: outcome.to_string(),
    }
}

/// Set aside the undelivered items of a queue whose record is being
/// archived, replaced or retired (#2028 v2: NO re-targeting). Pending
/// items are cancelled `stranded-archived` (or `stranded-expired` past
/// `not_after`); `inflight` items with no live claim owner follow, and
/// one that may already have been typed (`paste_started_at` or
/// `possibly_submitted`) is quarantined in `failed/` as
/// `possibly_submitted`. Nothing is ever filed `processed` and nothing
/// is re-enqueued: the caller raises the returned items in one
/// coalesced notice and a human re-sends. The queue's delivery lock is
/// taken without waiting; when a drain holds it, items are left for
/// that drain and the report is empty. Live claims are respected.
pub fn set_aside_archived_queue(root: &Path, now_ms: u64) -> ArchiveSetAsideReport {
    let mut report = ArchiveSetAsideReport::default();
    if fs::symlink_metadata(root).is_err() {
        return report;
    }
    let directories = QueueDirectories::new(root);
    let Ok(identity) = directory_identity(root) else {
        return report;
    };
    // Retry briefly once the current holder releases instead of
    // abandoning the sweep: a drain holds this lock only while it
    // moves items between phases.
    struct DeadlineWaiter(std::time::Instant);
    impl Waiter for DeadlineWaiter {
        fn cancelled(&self) -> bool {
            std::time::Instant::now() >= self.0
        }
        fn sleep(&self, duration: std::time::Duration) {
            std::thread::sleep(duration.min(std::time::Duration::from_millis(50)));
        }
    }
    let waiter = DeadlineWaiter(std::time::Instant::now() + std::time::Duration::from_secs(5));
    let Ok(_lock) = lock_delivery(root, identity, &waiter) else {
        return report;
    };
    let mut pending: Vec<(PathBuf, String)> = Vec::new();
    for directory in [&directories.inbox, &directories.inbox_v2] {
        if let Ok(ids) = identifiers_if_directory(directory) {
            for id in ids {
                pending.push((directory.join(format!("{id}.json")), id));
            }
        }
    }
    for (path, id) in pending {
        let Ok(document) = load_message(&path) else {
            continue;
        };
        let expired = document
            .get(NOT_AFTER)
            .filter(|value| !value.is_null())
            .and_then(Value::as_u64)
            .is_some_and(|deadline| deadline <= now_ms);
        let (reason, outcome) = if expired {
            ("stranded-expired", "stranded-expired")
        } else {
            ("stranded-archived", "stranded-archived")
        };
        let item = stranded_item(&document, &id, outcome);
        if set_aside(
            root,
            &directories,
            &path,
            document,
            reason,
            "coordinator archived; raised, not delivered",
        )
        .is_ok()
        {
            report.items.push(item);
        }
    }
    if let Ok(ids) = identifiers_if_directory(&directories.inflight) {
        for id in ids {
            let path = directories.inflight.join(format!("{id}.json"));
            let filename = path.file_name().expect("item filename").to_owned();
            if claim_owner_alive(&directories, &filename) {
                continue;
            }
            let Ok(mut document) = load_message(&path) else {
                continue;
            };
            let may_have_typed = document.get("paste_started_at").is_some()
                || document.get("possibly_submitted").and_then(Value::as_bool) == Some(true)
                || document.get("stranded_at").is_some()
                || document.get("typed_as").is_some();
            if may_have_typed {
                let item = stranded_item(&document, &id, "possibly_submitted");
                document.insert("possibly_submitted".to_owned(), Value::Bool(true));
                document.insert("delivery_state".to_owned(), json!("failed"));
                if atomic_json(&path, &Value::Object(document)).is_err() {
                    continue;
                }
                let failed_path = directories.failed.join(&filename);
                if transition(&path, &failed_path).is_ok() {
                    let sidecar = PathBuf::from(format!("{}.error", failed_path.display()));
                    let _ = fs::write(
                        &sidecar,
                        json!({"error": "archived while inflight; may have been typed, never replayed",
                               "failed_at": unix_seconds()}).to_string(),
                    );
                    remove_claim_lock(&directories, &filename);
                    report.items.push(item);
                }
            } else {
                let item = stranded_item(&document, &id, "stranded-archived");
                if set_aside(
                    root,
                    &directories,
                    &path,
                    document,
                    "stranded-archived",
                    "coordinator archived while inflight untyped; raised, not delivered",
                )
                .is_ok()
                {
                    report.items.push(item);
                }
            }
        }
    }
    report
}

/// The identifier an item's file name carries: the name without `.json`.
pub fn message_id_from_path(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".json"))
        .map(str::to_owned)
        .ok_or_else(|| {
            Error::invalid(format!(
                "queued message name is not valid UTF-8 JSON: {}",
                path.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::fsio::Blocking;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestQueue(PathBuf);

    impl TestQueue {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agent-queue-test-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            Self(path.join("queue"))
        }
    }

    impl Drop for TestQueue {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = fs::remove_dir_all(parent);
            }
        }
    }

    fn put(root: &Path, id: &str, serialize: bool) -> Result<String> {
        enqueue(root, Some(id), serialize, &Blocking, |id| {
            plain_item(id, "hello")
        })
    }

    #[test]
    fn a_lane_another_program_made_under_umask_022_is_reported_then_repaired_by_a_writer() {
        // As agent-teams once did: cancel/ created 0755 with a 0644 tombstone in it.
        let queue = TestQueue::new();
        let root = &queue.0;
        put(root, "a-1", true).unwrap();
        let cancel = root.join(CANCEL);
        fs::create_dir(&cancel).unwrap();
        fs::set_permissions(&cancel, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(cancel.join("other.json"), b"{}").unwrap();

        // A read-only lookup changes nothing and says what to do.
        let error = message_state(root, "a-1").unwrap_err().to_string();
        assert!(
            error.contains("queue state directory is not private"),
            "{error}"
        );
        assert!(error.contains("mode 0755"), "{error}");
        assert!(error.contains("chmod 700"), "{error}");
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&cancel), 0o755);

        // The next writer repairs it, and lookups work again.
        put(root, "a-2", false).unwrap();
        assert_eq!(mode(&cancel), 0o700);
        assert_eq!(
            message_state(root, "a-1").unwrap(),
            Some(MessageState::Pending)
        );

        // A lane others can write to is never repaired silently.
        fs::set_permissions(&cancel, fs::Permissions::from_mode(0o777)).unwrap();
        let error = put(root, "a-3", false).unwrap_err().to_string();
        assert!(error.contains("group or others can write"), "{error}");
        assert_eq!(mode(&cancel), 0o777);
    }

    #[test]
    fn archive_set_aside_raises_pending_and_expired_never_processed() {
        // #2028 v2: pending items are set aside raised, expired apart,
        // nothing re-enqueued, nothing processed.
        let queue = TestQueue::new();
        let root = &queue.0;
        enqueue_into(root, Lane::InboxV2, Some("keep-1"), true, &Blocking, |id| {
            let mut doc = plain_item(id, "hello world");
            doc["not_after"] = json!(4_102_444_800_000u64);
            doc
        })
        .unwrap();
        enqueue_into(root, Lane::InboxV2, Some("old-1"), true, &Blocking, |id| {
            let mut doc = plain_item(id, "stale");
            doc["not_after"] = json!(1u64);
            doc
        })
        .unwrap();
        let report = set_aside_archived_queue(root, 1_700_000_000_000);
        assert_eq!(report.items.len(), 2, "{report:?}");
        assert_eq!(
            message_state(root, "keep-1").unwrap(),
            Some(MessageState::Cancelled)
        );
        assert_eq!(
            message_state(root, "old-1").unwrap(),
            Some(MessageState::Cancelled)
        );
        let kept = load_message(&root.join("cancelled/keep-1.json")).unwrap();
        assert_eq!(kept["cancel_reason"], "stranded-archived");
        let old = load_message(&root.join("cancelled/old-1.json")).unwrap();
        assert_eq!(old["cancel_reason"], "stranded-expired");
        assert!(report
            .items
            .iter()
            .all(|item| item.outcome.starts_with("stranded")));
    }

    #[test]
    fn enqueue_creates_a_private_queue_and_refuses_an_identifier_in_any_phase() {
        let queue = TestQueue::new();
        let root = &queue.0;
        assert_eq!(put(root, "a-1", true).unwrap(), "a-1");
        let mode = fs::metadata(root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let item = load_message(&root.join("inbox/a-1.json")).unwrap();
        assert_eq!(item["id"], "a-1");
        assert_eq!(item["delivery_attempts"], 0);
        assert!(item["queued_at"].as_f64().is_some());
        assert_eq!(
            message_state(root, "a-1").unwrap(),
            Some(MessageState::Pending)
        );

        for (phase, state) in [
            (INFLIGHT, MessageState::Inflight),
            (PROCESSED, MessageState::Processed),
            (FAILED, MessageState::Failed),
        ] {
            let id = format!("in-{phase}");
            put(root, &id, false).unwrap();
            transition(
                &root.join(INBOX).join(format!("{id}.json")),
                &root.join(phase).join(format!("{id}.json")),
            )
            .unwrap();
            assert_eq!(message_state(root, &id).unwrap(), Some(state));
            for serialize in [true, false] {
                let error = put(root, &id, serialize).unwrap_err();
                assert_eq!(error.kind(), ErrorKind::Duplicate, "{error}");
                assert_eq!(
                    error.to_string(),
                    format!("message id already exists: {id}")
                );
            }
        }
        assert_eq!(message_state(root, "never").unwrap(), None);
        assert_eq!(pending_identifiers(root).unwrap(), vec!["a-1"]);
    }

    #[test]
    fn invalid_identifiers_and_missing_queues_are_handled_without_writing() {
        let queue = TestQueue::new();
        for bad in ["", ".hidden", "a/b", "a b", &"x".repeat(MESSAGE_ID_MAX + 1)] {
            assert_eq!(
                validate_message_id(bad).unwrap_err().kind(),
                ErrorKind::Invalid
            );
        }
        validate_message_id(&"x".repeat(MESSAGE_ID_MAX)).unwrap();
        let generated = generated_message_id();
        validate_message_id(&generated).unwrap();
        assert_eq!(message_state(&queue.0, "a").unwrap(), None);
        for document in [
            json!({"text": ""}),
            json!({"id": "other", "text": "t"}),
            json!("t"),
        ] {
            let error =
                enqueue(&queue.0, Some("a"), true, &Blocking, |_| document.clone()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Invalid, "{error}");
        }
        assert!(pending_identifiers(&queue.0).unwrap().is_empty());
        std::fs::remove_dir_all(&queue.0).unwrap();
        assert!(pending_identifiers(&queue.0).unwrap().is_empty());
        assert!(!withdraw(&queue.0, "a", &Blocking).unwrap());
        assert!(!queue.0.exists());
    }

    #[test]
    fn withdraw_removes_only_an_inbox_item_so_its_identifier_can_be_reused() {
        let queue = TestQueue::new();
        let root = &queue.0;
        put(root, "w", true).unwrap();
        assert!(withdraw(root, "w", &Blocking).unwrap());
        assert!(!withdraw(root, "w", &Blocking).unwrap());
        put(root, "w", true).unwrap();
        transition(&root.join("inbox/w.json"), &root.join("processed/w.json")).unwrap();
        assert!(!withdraw(root, "w", &Blocking).unwrap());
        assert_eq!(
            message_state(root, "w").unwrap(),
            Some(MessageState::Processed)
        );
    }

    #[test]
    fn a_held_delivery_lock_blocks_a_serialized_enqueue_until_the_waiter_gives_up() {
        struct GiveUp;
        impl Waiter for GiveUp {
            fn cancelled(&self) -> bool {
                true
            }
            fn sleep(&self, _duration: std::time::Duration) {}
        }
        let queue = TestQueue::new();
        let root = &queue.0;
        prepare(root).unwrap();
        let _held = lock_delivery(root, directory_identity(root).unwrap(), &Blocking).unwrap();
        // flock is per open file description, so a second open in this process contends.
        let error = enqueue(root, Some("x"), true, &GiveUp, |id| plain_item(id, "t")).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Cancelled);
        assert!(enqueue(root, None, false, &GiveUp, |id| plain_item(id, "t")).is_ok());
        let error = withdraw(root, "x", &GiveUp).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Cancelled);
    }

    #[test]
    fn queue_order_is_by_queued_at_then_name_with_untimed_items_first() {
        let queue = TestQueue::new();
        let directories = prepare(&queue.0).unwrap();
        let write = |name: &str, queued_at: Value| {
            atomic_json(
                &directories.inbox.join(format!("{name}.json")),
                &json!({"text": "t", "queued_at": queued_at}),
            )
            .unwrap();
        };
        write("c", json!(2.0));
        write("a", json!(3.0));
        write("b", json!(2.0));
        write("z", json!("text"));
        write("y", Value::Null);
        write("n", json!(-0.0));
        write("m", json!(0.0));
        let order: Vec<String> = inbox_in_queue_order(&directories.inbox)
            .unwrap()
            .iter()
            .map(|path| message_id_from_path(path).unwrap())
            .collect();
        assert_eq!(order, ["y", "z", "m", "n", "b", "c", "a"]);
    }

    #[test]
    fn the_second_lane_is_created_on_first_use_and_shares_the_identifier_space() {
        let queue = TestQueue::new();
        let root = &queue.0;
        put(root, "old", true).unwrap();
        let directories = QueueDirectories::new(root);
        assert!(
            !directories.uses_lanes(),
            "a v1 queue grows no lane directories"
        );
        let error = enqueue_into(
            root,
            Lane::Inbox,
            Some("x"),
            true,
            &Blocking,
            |id| json!({"id": id, "text": "t", "not_after": 5}),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Invalid, "{error}");
        let error = enqueue_into(
            root,
            Lane::InboxV2,
            Some("x"),
            true,
            &Blocking,
            |id| json!({"id": id, "text": "t", "not_after": "soon"}),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Invalid, "{error}");
        enqueue_into(
            root,
            Lane::InboxV2,
            Some("new"),
            false,
            &Blocking,
            |id| json!({"id": id, "text": "t", "queued_at": 0.5, "not_after": 9}),
        )
        .unwrap();
        assert!(directories.uses_lanes());
        let mode = fs::metadata(&directories.inbox_v2)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        assert_eq!(
            message_state(root, "new").unwrap(),
            Some(MessageState::Pending)
        );
        for (lane, id) in [(Lane::Inbox, "new"), (Lane::InboxV2, "old")] {
            let error = enqueue_into(root, lane, Some(id), false, &Blocking, |id| {
                plain_item(id, "t")
            })
            .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Duplicate);
        }
        assert_eq!(pending_identifiers(root).unwrap(), ["new", "old"]);
        let order: Vec<String> = pending_in_queue_order(&directories)
            .unwrap()
            .iter()
            .map(|path| message_id_from_path(path).unwrap())
            .collect();
        assert_eq!(order, ["new", "old"], "merged by queued_at across lanes");
        assert!(withdraw(root, "new", &Blocking).unwrap());
        assert_eq!(message_state(root, "new").unwrap(), None);
    }

    #[test]
    fn a_named_id_in_a_terminal_phase_is_never_reenqueued_and_cancel_resolves_phases() {
        // #2022 cancel-resurrection, named-id form: a re-send with an
        // explicit id whose record is already cancelled is refused, and
        // cancel on an id in several phases retires the pending copy
        // instead of refusing on the ambiguity.
        let queue = TestQueue::new();
        let root = &queue.0;
        let directories = prepare(root).unwrap();
        put(root, "named-1", true).unwrap();
        assert_eq!(
            cancel(root, "named-1", "delivered by hand").unwrap(),
            CancelOutcome::SetAside
        );
        assert_eq!(
            put(root, "named-1", true).unwrap_err().kind(),
            ErrorKind::Duplicate,
            "a cancelled named id is never re-enqueued"
        );
        // A resurrected pending copy beside the cancelled record: cancel
        // resolves it, keeping the terminal record.
        let pending = directories.inbox.join("named-1.json");
        fs::copy(directories.cancelled.join("named-1.json"), &pending).unwrap();
        assert_eq!(
            cancel(root, "named-1", "again").unwrap(),
            CancelOutcome::AlreadyCancelled
        );
        assert!(!pending.exists(), "the pending copy was retired");
        assert!(directories.cancelled.join("named-1.json").exists());
        // Cancelled (via set_aside, tombstone consumed) plus a revived
        // pending copy: cancel keeps the terminal record and retires it.
        put(root, "named-2", true).unwrap();
        let claimed = directories.inbox.join("named-2.json");
        let document = load_message(&claimed).unwrap();
        set_aside(root, &directories, &claimed, document, "cancelled", "x").unwrap();
        let revived = directories.inbox.join("named-2.json");
        fs::copy(directories.cancelled.join("named-2.json"), &revived).unwrap();
        assert_eq!(
            cancel(root, "named-2", "again").unwrap(),
            CancelOutcome::AlreadyCancelled
        );
        assert!(!revived.exists());
    }

    #[test]
    fn a_tombstone_or_a_passed_deadline_sets_an_item_aside_untyped() {
        let queue = TestQueue::new();
        let root = &queue.0;
        let directories = prepare(root).unwrap();
        enqueue_into(
            root,
            Lane::InboxV2,
            Some("late"),
            true,
            &Blocking,
            |id| json!({"id": id, "text": "t", "not_after": 1_000}),
        )
        .unwrap();
        put(root, "kept", true).unwrap();
        let item = |id: &str, lane: &Path| load_message(&lane.join(format!("{id}.json"))).unwrap();
        assert_eq!(
            set_aside_reason(
                &directories,
                "late",
                &item("late", &directories.inbox_v2),
                999
            ),
            None
        );
        enqueue_into(
            root,
            Lane::InboxV2,
            Some("open"),
            false,
            &Blocking,
            |id| json!({"id": id, "text": "t", "not_after": null}),
        )
        .unwrap();
        assert_eq!(
            set_aside_reason(
                &directories,
                "open",
                &item("open", &directories.inbox_v2),
                u64::MAX
            ),
            None,
            "a null not_after is no deadline"
        );
        let (reason, detail) = set_aside_reason(
            &directories,
            "late",
            &item("late", &directories.inbox_v2),
            1_000,
        )
        .unwrap();
        assert_eq!(reason, "expired");
        assert!(detail.contains("1000"), "{detail}");

        // While a drain holds the queue the tombstone is all cancel can write.
        let held = lock_delivery(root, directory_identity(root).unwrap(), &Blocking).unwrap();
        assert_eq!(
            cancel(root, "kept", "superseded").unwrap(),
            CancelOutcome::Pending
        );
        assert_eq!(
            cancel(root, "kept", "again").unwrap(),
            CancelOutcome::Pending
        );
        drop(held);
        assert_eq!(
            set_aside_reason(&directories, "kept", &item("kept", &directories.inbox), 0),
            Some(("cancelled", "superseded".to_owned()))
        );
        let path = directories.inbox.join("kept.json");
        let document = item("kept", &directories.inbox);
        assert_eq!(
            set_aside(
                root,
                &directories,
                &path,
                document,
                "cancelled",
                "superseded"
            )
            .unwrap(),
            "kept"
        );
        assert_eq!(
            message_state(root, "kept").unwrap(),
            Some(MessageState::Cancelled)
        );
        let moved = load_message(&directories.cancelled.join("kept.json")).unwrap();
        assert_eq!(moved["delivery_state"], "cancelled");
        assert_eq!(moved["cancel_reason"], "cancelled");
        assert_eq!(
            cancel(root, "kept", "x").unwrap(),
            CancelOutcome::AlreadyCancelled
        );
        // A copy already in cancelled/ (retired by hand, no tombstone:
        // #2022) sets a same-id pending item aside too.
        assert_eq!(
            set_aside_reason(
                &directories,
                "kept",
                &item("kept", &directories.cancelled),
                0
            ),
            Some(("cancelled", "already retired".to_owned()))
        );
        assert_eq!(
            put(root, "kept", true).unwrap_err().kind(),
            ErrorKind::Duplicate
        );

        assert_eq!(
            cancel(root, "future", "retract").unwrap(),
            CancelOutcome::Unknown
        );
        // A later item under that identifier is set aside by the waiting tombstone.
        assert!(directories.cancel.join("future.json").exists());
        // With no drain holding the queue, a waiting item is set aside at once and its
        // tombstone is consumed.
        put(root, "now", true).unwrap();
        assert_eq!(
            cancel(root, "now", "lifted").unwrap(),
            CancelOutcome::SetAside
        );
        assert_eq!(
            message_state(root, "now").unwrap(),
            Some(MessageState::Cancelled)
        );
        assert!(!directories.cancel.join("now.json").exists());
        put(root, "done", true).unwrap();
        transition(
            &directories.inbox.join("done.json"),
            &directories.processed.join("done.json"),
        )
        .unwrap();
        assert_eq!(cancel(root, "done", "x").unwrap(), CancelOutcome::TooLate);
        assert!(!directories.cancel.join("done.json").exists());
    }

    #[test]
    fn a_supersede_key_replaces_only_pending_items_with_the_same_key() {
        let queue = TestQueue::new();
        let root = &queue.0;
        let directories = prepare(root).unwrap();
        let keyed = |id: &str, queued_at: f64, extra: Value| {
            let mut document = json!({
                "id": id, "text": "t", "queued_at": queued_at, "delivery_attempts": 0,
                "supersede_key": "notice:lane-a",
            });
            document
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().cloned().unwrap_or_default());
            enqueue_into_outcome(root, Lane::InboxV2, Some(id), true, &Blocking, move |_| {
                document
            })
            .unwrap()
        };
        // Settled history and a composer-owned copy with the same key, one plain pending
        // copy, and an unkeyed neighbour. Setup enqueues go settled-first: each keyed
        // enqueue supersedes plain pending copies, so order builds the intended set.
        keyed("inflight-keyed", 101.0, json!({}));
        transition(
            &directories.inbox_v2.join("inflight-keyed.json"),
            &directories.inflight.join("inflight-keyed.json"),
        )
        .unwrap();
        keyed("done-keyed", 102.0, json!({}));
        transition(
            &directories.inbox_v2.join("done-keyed.json"),
            &directories.processed.join("done-keyed.json"),
        )
        .unwrap();
        keyed("stranded-keyed", 103.0, json!({"stranded_at": 103.5}));
        enqueue_into(
            root,
            Lane::Inbox,
            Some("unkeyed"),
            true,
            &Blocking,
            |id| json!({"id": id, "text": "t", "queued_at": 150.0, "delivery_attempts": 0}),
        )
        .unwrap();
        keyed("first", 100.0, json!({}));

        let outcome = keyed("newest", 200.0, json!({}));
        assert_eq!(outcome.id, "newest");
        assert_eq!(
            outcome.superseded,
            vec!["first"],
            "only the plain pending copy"
        );
        assert_eq!(
            message_state(root, "first").unwrap(),
            Some(MessageState::Cancelled)
        );
        let replaced = load_message(&directories.cancelled.join("first.json")).unwrap();
        assert_eq!(replaced["cancel_reason"], "superseded");
        assert_eq!(replaced["superseded_by"], "newest");
        assert_eq!(replaced["cancel_detail"], "superseded by newest");
        // Delivered history and composer-owned items are never touched.
        assert_eq!(
            message_state(root, "done-keyed").unwrap(),
            Some(MessageState::Processed)
        );
        assert_eq!(
            message_state(root, "inflight-keyed").unwrap(),
            Some(MessageState::Inflight)
        );
        assert_eq!(
            message_state(root, "stranded-keyed").unwrap(),
            Some(MessageState::Pending)
        );
        assert_eq!(
            message_state(root, "unkeyed").unwrap(),
            Some(MessageState::Pending)
        );
        // The replacement keeps its own fresh position: last by queued_at.
        let order: Vec<String> = pending_in_queue_order(&directories)
            .unwrap()
            .iter()
            .map(|path| message_id_from_path(path).unwrap())
            .collect();
        assert_eq!(
            order.last().map(String::as_str),
            Some("newest"),
            "{order:?}"
        );
        assert!(!order.contains(&"first".to_owned()), "{order:?}");

        // A key without the serializing lock, or of the wrong shape, writes nothing.
        let error = enqueue_into_outcome(
            root,
            Lane::InboxV2,
            Some("loose"),
            false,
            &Blocking,
            |id| json!({"id": id, "text": "t", "supersede_key": "notice:lane-a"}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("serialized"), "{error}");
        assert_eq!(message_state(root, "loose").unwrap(), None);
        let error = enqueue_into_outcome(
            root,
            Lane::InboxV2,
            Some("bad-key"),
            true,
            &Blocking,
            |id| json!({"id": id, "text": "t", "supersede_key": "with\nnewline"}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("supersede key"), "{error}");
        assert_eq!(message_state(root, "bad-key").unwrap(), None);
        // An entry whose key field is not a string never matches and is left alone.
        enqueue_into(
            root,
            Lane::Inbox,
            Some("odd"),
            true,
            &Blocking,
            |id| json!({"id": id, "text": "t", "supersede_key": 7}),
        )
        .unwrap_err();
        assert_eq!(message_state(root, "odd").unwrap(), None);
    }

    #[test]
    fn recovery_quarantines_every_inflight_item_as_possibly_submitted() {
        let queue = TestQueue::new();
        let directories = prepare(&queue.0).unwrap();
        put(&queue.0, "r", true).unwrap();
        transition(
            &directories.inbox.join("r.json"),
            &directories.inflight.join("r.json"),
        )
        .unwrap();
        assert_eq!(recover_inflight(&directories).unwrap(), vec!["r"]);
        assert_eq!(
            message_state(&queue.0, "r").unwrap(),
            Some(MessageState::Failed)
        );
        let sidecar = read_json(&directories.failed.join("r.json.error")).unwrap();
        assert_eq!(sidecar["outcome"], "possibly_submitted");
        assert_eq!(sidecar["artifact"], "r.json");
        let error = transition(
            &directories.failed.join("r.json"),
            &directories.failed.join("r.json"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Duplicate);
    }

    #[test]
    fn recovery_repends_tracked_items_that_were_never_typed() {
        let queue = TestQueue::new();
        let directories = prepare(&queue.0).unwrap();
        // A tracked item from inbox.v2 whose paste never started goes back.
        enqueue_into(
            &queue.0,
            Lane::InboxV2,
            Some("never"),
            true,
            &Blocking,
            |id| plain_item(id, "text"),
        )
        .unwrap();
        let path = directories.inbox_v2.join("never.json");
        let mut document = load_message(&path).unwrap();
        document.insert("possibly_submitted".to_owned(), json!(true));
        document.insert("paste_tracking".to_owned(), json!(true));
        document.insert("inflight_from".to_owned(), json!("inbox.v2"));
        atomic_json(&path, &Value::Object(document)).unwrap();
        transition(&path, &directories.inflight.join("never.json")).unwrap();
        // A tracked item whose paste started stays quarantined.
        put(&queue.0, "pasted", true).unwrap();
        let path = directories.inbox.join("pasted.json");
        let mut document = load_message(&path).unwrap();
        document.insert("possibly_submitted".to_owned(), json!(true));
        document.insert("paste_tracking".to_owned(), json!(true));
        document.insert("inflight_from".to_owned(), json!("inbox"));
        document.insert("paste_started_at".to_owned(), json!(1.0));
        atomic_json(&path, &Value::Object(document)).unwrap();
        transition(&path, &directories.inflight.join("pasted.json")).unwrap();

        assert_eq!(recover_inflight(&directories).unwrap(), vec!["pasted"]);
        assert_eq!(
            message_state(&queue.0, "never").unwrap(),
            Some(MessageState::Pending)
        );
        let back = load_message(&directories.inbox_v2.join("never.json")).unwrap();
        assert!(back.get("possibly_submitted").is_none(), "{back:?}");
        assert!(back.get("paste_tracking").is_none(), "{back:?}");
        assert_eq!(
            message_state(&queue.0, "pasted").unwrap(),
            Some(MessageState::Failed)
        );
    }

    #[test]
    fn a_fresh_claim_is_left_for_its_owner_and_a_stale_one_recovers() {
        // #1480: a drain claims an item (inflight + fresh delivery_claimed_at)
        // then releases the queue lock to type. A concurrent drain's recovery
        // must leave that live claim alone — never lost, never typed twice —
        // while a stale claim (its drainer died between claim and type)
        // recovers exactly as before: never-typed back to pending, pasted to
        // failed as possibly_submitted.
        let queue = TestQueue::new();
        let directories = prepare(&queue.0).unwrap();
        let claim = |id: &str, claimed_at: f64, pasted: bool| {
            put(&queue.0, id, true).unwrap();
            let path = directories.inbox.join(format!("{id}.json"));
            let mut document = load_message(&path).unwrap();
            document.insert("possibly_submitted".to_owned(), json!(true));
            document.insert("paste_tracking".to_owned(), json!(true));
            document.insert("inflight_from".to_owned(), json!("inbox"));
            document.insert("inflight_at".to_owned(), json!(claimed_at));
            document.insert("delivery_claimed_at".to_owned(), json!(claimed_at));
            if pasted {
                document.insert("paste_started_at".to_owned(), json!(claimed_at));
            }
            atomic_json(&path, &Value::Object(document)).unwrap();
            transition(&path, &directories.inflight.join(format!("{id}.json"))).unwrap();
        };
        let now = unix_seconds();
        claim("fresh", now, false);
        // The live owner holds the fresh item's claim lock; stale claims have none.
        let _fresh_owner = take_claim_lock(
            &directories,
            directories.inflight.join("fresh.json").file_name().unwrap(),
        )
        .unwrap()
        .expect("the fresh claim lock is free to take for its owner");
        claim("stale-new", now - 3600.0, false);
        claim("stale-pasted", now - 3600.0, true);
        assert_eq!(
            recover_inflight(&directories).unwrap(),
            vec!["stale-pasted"]
        );
        assert_eq!(
            message_state(&queue.0, "fresh").unwrap(),
            Some(MessageState::Inflight)
        );
        assert_eq!(
            message_state(&queue.0, "stale-new").unwrap(),
            Some(MessageState::Pending)
        );
        assert_eq!(
            message_state(&queue.0, "stale-pasted").unwrap(),
            Some(MessageState::Failed)
        );
        // The fresh claim is still its owner's to finish: once its owner is
        // gone (claim lock released) it recovers as never-typed, not as
        // possibly submitted.
        drop(_fresh_owner);
        assert_eq!(
            recover_inflight(&directories).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            message_state(&queue.0, "fresh").unwrap(),
            Some(MessageState::Pending)
        );
    }

    #[test]
    fn stranded_recovery_repends_items_whose_paste_started() {
        // #884 item 4 (D): an item a drainer may have typed goes back to
        // pending as stranded — its paste may still be in the composer for
        // the next drain to flush with Enter — never straight to failed/.
        let queue = TestQueue::new();
        let directories = prepare(&queue.0).unwrap();
        put(&queue.0, "pasted", true).unwrap();
        let path = directories.inbox.join("pasted.json");
        let mut document = load_message(&path).unwrap();
        document.insert("possibly_submitted".to_owned(), json!(true));
        document.insert("paste_tracking".to_owned(), json!(true));
        document.insert("inflight_from".to_owned(), json!("inbox"));
        document.insert("paste_started_at".to_owned(), json!(123.0));
        atomic_json(&path, &Value::Object(document)).unwrap();
        transition(&path, &directories.inflight.join("pasted.json")).unwrap();
        // An untracked item is left for the ordinary quarantine.
        put(&queue.0, "legacy", true).unwrap();
        transition(
            &directories.inbox.join("legacy.json"),
            &directories.inflight.join("legacy.json"),
        )
        .unwrap();

        assert_eq!(
            repend_stranded_inflight(&directories).unwrap(),
            vec!["pasted"]
        );
        assert_eq!(
            message_state(&queue.0, "pasted").unwrap(),
            Some(MessageState::Pending)
        );
        let back = load_message(&directories.inbox.join("pasted.json")).unwrap();
        assert_eq!(back["stranded_at"], json!(123.0), "{back:?}");
        assert!(back.get("paste_started_at").is_none(), "{back:?}");
        assert!(back.get("possibly_submitted").is_none(), "{back:?}");
        assert_eq!(recover_inflight(&directories).unwrap(), vec!["legacy"]);
    }
}
