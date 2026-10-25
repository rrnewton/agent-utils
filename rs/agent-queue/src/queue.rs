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
/// The lock that serializes drains, explicit-identifier enqueues and withdrawals.
pub const DELIVERY_LOCK: &str = ".delivery.lock";
/// The lock that serializes changes to the target binding.
pub const BINDING_LOCK: &str = ".binding.lock";
/// The target binding, written by the first send.
pub const TARGET_BINDING: &str = "target.json";

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
        }
    }

    /// Every phase directory, in lifecycle order.
    #[must_use]
    pub fn all(&self) -> [&Path; 4] {
        [&self.inbox, &self.inflight, &self.processed, &self.failed]
    }

    /// Every phase directory with the state an item in it has.
    #[must_use]
    pub fn states(&self) -> [(&Path, MessageState); 4] {
        [
            (&self.inbox, MessageState::Pending),
            (&self.inflight, MessageState::Inflight),
            (&self.processed, MessageState::Processed),
            (&self.failed, MessageState::Failed),
        ]
    }
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
    for directory in QueueDirectories::new(root).all() {
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
/// item from its identifier once the check has passed.
pub fn enqueue(
    root: &Path,
    message_id: Option<&str>,
    serialize: bool,
    waiter: &dyn Waiter,
    document: impl FnOnce(&str) -> Value,
) -> Result<String> {
    let directories = prepare(root)?;
    let identifier = message_id.map_or_else(generated_message_id, str::to_owned);
    validate_message_id(&identifier)?;
    let filename = format!("{identifier}.json");
    let path = directories.inbox.join(&filename);
    let _lock = if serialize {
        Some(lock_delivery(root, directory_identity(root)?, waiter)?)
    } else {
        None
    };
    if directories
        .all()
        .iter()
        .any(|directory| fs::symlink_metadata(directory.join(&filename)).is_ok())
    {
        return Err(duplicate(&identifier));
    }
    atomic_json_create(&path, &document(&identifier)).map_err(|error| {
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

/// The identifiers in the queue's inbox, sorted by name; empty when the queue does not exist.
pub fn pending_identifiers(root: &Path) -> Result<Vec<String>> {
    if fs::symlink_metadata(root).is_err() {
        return Ok(Vec::new());
    }
    validate_existing_queue(root)?;
    identifiers_if_directory(&QueueDirectories::new(root).inbox)
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
    let inbox = QueueDirectories::new(root).inbox;
    let path = inbox.join(format!("{message_id}.json"));
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error("inspect queue message", &path, error)),
    }
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

/// Quarantine every item left in `inflight/` as `possibly_submitted`, never replaying it, and
/// return their identifiers. Only the holder of the delivery lock may call this.
pub fn recover_inflight(directories: &QueueDirectories) -> Result<Vec<String>> {
    let mut recovered = Vec::new();
    for path in json_paths(&directories.inflight)? {
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
    let mut entries: Vec<(Option<f64>, PathBuf)> = json_paths(inbox)?
        .into_iter()
        .map(|path| {
            let queued_at = read_json(&path)
                .ok()
                .and_then(|document| document.get("queued_at").and_then(Value::as_f64))
                // `total_cmp` puts -0.0 before 0.0; make them one time.
                .map(|seconds| if seconds == 0.0 { 0.0 } else { seconds });
            (queued_at, path)
        })
        .collect();
    entries.sort_by(|(left_at, left), (right_at, right)| {
        match (left_at, right_at) {
            (Some(left_time), Some(right_time)) => left_time.total_cmp(right_time),
            _ => left_at.is_some().cmp(&right_at.is_some()),
        }
        .then_with(|| left.cmp(right))
    });
    Ok(entries.into_iter().map(|(_, path)| path).collect())
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
}
