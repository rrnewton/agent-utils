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
    read_json, sync_directory, transition, validate_private_directory, Waiter,
};

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
    sync_directory(root)?;
    Ok(directories)
}

/// Require an existing queue and its existing phase directories to be private; a queue or
/// phase that does not exist yet passes.
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
    Ok(identifier)
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
    json!({
        "id": identifier,
        "text": text,
        "queued_at": unix_seconds(),
        "delivery_attempts": 0,
    })
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
        &json!({
            "artifact": filename,
            "outcome": outcome,
            "error": error,
            "failed_at": unix_seconds(),
        }),
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
    let Ok(mut document) = load_message(path) else {
        return Ok(false);
    };
    if document.get("paste_tracking") != Some(&Value::Bool(true))
        || document.contains_key("paste_started_at")
    {
        return Ok(false);
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
    ] {
        document.remove(field);
    }
    document.insert("delivery_state".to_owned(), json!("pending"));
    atomic_json(path, &Value::Object(document))?;
    let filename = path.file_name().expect("listed path has filename");
    transition(path, &lane.join(filename))?;
    Ok(true)
}

/// Quarantine every item left in `inflight/` as `possibly_submitted`, never replaying it, and
/// return their identifiers. An item a tracking drainer provably never
/// typed is returned to its pending lane instead ([`repend_never_typed`])
/// and is not returned. Only the holder of the delivery lock may call this.
pub fn recover_inflight(directories: &QueueDirectories) -> Result<Vec<String>> {
    let mut recovered = Vec::new();
    for path in json_paths(&directories.inflight)? {
        if repend_never_typed(directories, &path)? {
            continue;
        }
        let filename = path.file_name().expect("listed path has filename");
        let destination = directories.failed.join(filename);
        transition(&path, &destination)?;
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
    match settled_state(root, message_id)? {
        Some(MessageState::Cancelled) => return Ok(CancelOutcome::AlreadyCancelled),
        Some(MessageState::Processed | MessageState::Failed) => return Ok(CancelOutcome::TooLate),
        _ => {}
    }
    prepare_lane(root, &directories.cancel)?;
    let path = directories.cancel.join(format!("{message_id}.json"));
    let tombstone = json!({"id": message_id, "reason": reason, "cancelled_at": unix_seconds()});
    match atomic_json_create(&path, &tombstone) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error("write queue tombstone", &path, error)),
    }
    // With the delivery lock free, no drain is between its last check and its paste, so the item
    // can be set aside now, out of reach of every drainer.
    if let Ok(_lock) = lock_delivery(root, directory_identity(root)?, &NoWait) {
        for inbox in [&directories.inbox, &directories.inbox_v2] {
            let item = inbox.join(format!("{message_id}.json"));
            if fs::symlink_metadata(&item).is_ok() {
                let document = load_message(&item)?;
                set_aside(root, &directories, &item, document, "cancelled", reason)?;
                return Ok(CancelOutcome::SetAside);
            }
        }
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
/// `("cancelled", reason)` when a tombstone names it, `("expired", detail)` when its `not_after`
/// has passed, or `None`. Drainers ask this just before typing.
pub fn set_aside_reason(
    directories: &QueueDirectories,
    message_id: &str,
    document: &Map<String, Value>,
    now_millis: u64,
) -> Option<(&'static str, String)> {
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
    document.insert("delivery_state".to_owned(), json!("cancelled"));
    document.insert("cancel_reason".to_owned(), json!(reason));
    document.insert("cancel_detail".to_owned(), json!(detail));
    document.insert("cancelled_at".to_owned(), json!(unix_seconds()));
    atomic_json(path, &Value::Object(document))?;
    transition(path, &directories.cancelled.join(filename))?;
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
}
