//! Durable first-choice binding for rejection reactions, separate from the posting ledger.

use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use super::{exact_keys, valid_operation_uuid, OutboundConfig, Refusal};
use crate::diag::Diagnostics;
use crate::ids::{self, AppId};
use crate::transport::{Outcome, SlackTransport, TransportError};

const SCHEMA: &str = "agentctl.slack-rejection.v1";
const MAX_RECORD_BYTES: u64 = 4 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Record {
    id: String,
    app_id: String,
    channel_id: String,
    message_ts: String,
    sender_id: String,
    name: String,
    complete: bool,
}

impl Record {
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        let invalid = || io::Error::other("invalid rejection journal record");
        let value =
            chat_subscription_plugin::decode_strict_json::<Value>(bytes).map_err(|_| invalid())?;
        let object = value.as_object().ok_or_else(invalid)?;
        if !exact_keys(
            object,
            &[
                "schema",
                "id",
                "app_id",
                "channel_id",
                "message_ts",
                "sender_id",
                "name",
                "complete",
            ],
        ) || object.get("schema").and_then(Value::as_str) != Some(SCHEMA)
        {
            return Err(invalid());
        }
        let text = |key: &str| -> io::Result<String> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(invalid)
        };
        let record = Self {
            id: text("id")?,
            app_id: text("app_id")?,
            channel_id: text("channel_id")?,
            message_ts: text("message_ts")?,
            sender_id: text("sender_id")?,
            name: text("name")?,
            complete: object
                .get("complete")
                .and_then(Value::as_bool)
                .ok_or_else(invalid)?,
        };
        if !valid_operation_uuid(&record.id)
            || AppId::new(record.app_id.clone()).is_err()
            || !ids::valid_channel_id(&record.channel_id)
            || !ids::valid_ts(&record.message_ts)
            || !ids::valid_user_id(&record.sender_id)
            || ids::reaction_name(&record.name).as_deref() != Some(record.name.as_str())
        {
            return Err(invalid());
        }
        Ok(record)
    }

    fn encoded(&self) -> io::Result<Vec<u8>> {
        serde_json::to_vec(&json!({
            "schema": SCHEMA, "id": self.id, "app_id": self.app_id,
            "channel_id": self.channel_id, "message_ts": self.message_ts,
            "sender_id": self.sender_id, "name": self.name, "complete": self.complete,
        }))
        .map_err(io::Error::other)
    }

    fn receipt(&self, already_present: bool) -> Value {
        json!({
            "reaction_id": format!("{}/reactions/{}", ids::message_resource(&self.channel_id, &self.message_ts), self.name),
            "already_present": already_present,
        })
    }
}

struct Journal {
    directory: File,
    // Keep the independent open-file-description lock until all reads, writes and Slack calls end.
    _lock: File,
    filename: CString,
}

impl Journal {
    fn open(root: &Path, id: &str) -> Result<Self, Refusal> {
        let directory = private_directory(root)
            .and_then(|root| child_directory(&root, c"rejections"))
            .and_then(|directory| {
                require_private_directory(&directory)?;
                Ok(directory)
            })
            .map_err(unreadable)?;
        let lock_name = name(&format!("{id}.lock")).map_err(unreadable)?;
        let lock = open_at(
            &directory,
            &lock_name,
            libc::O_RDWR | libc::O_CREAT | libc::O_NONBLOCK,
        )
        .and_then(|file| {
            require_private_file(&file, 0)?;
            Ok(file)
        })
        .map_err(unreadable)?;
        if let Err(error) = try_lock(&lock) {
            return Err(if error.kind() == io::ErrorKind::WouldBlock {
                Refusal::unknown(
                    "rejection_ledger_busy",
                    "another helper is handling this rejection operation; retry the same UUID",
                )
            } else {
                unreadable(error)
            });
        }
        Ok(Self {
            directory,
            _lock: lock,
            filename: name(&format!("{id}.json")).map_err(unreadable)?,
        })
    }

    fn load(&self) -> Result<Option<Record>, Refusal> {
        let file = match open_at(
            &self.directory,
            &self.filename,
            libc::O_RDONLY | libc::O_NONBLOCK,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(error)),
        };
        require_private_file(&file, MAX_RECORD_BYTES).map_err(unreadable)?;
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(unreadable(io::Error::other(
                "oversized rejection journal record",
            )));
        }
        Record::decode(&bytes).map(Some).map_err(unreadable)
    }

    fn save(&self, record: &Record) -> io::Result<()> {
        let bytes = record.encoded()?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(io::Error::other("oversized rejection journal record"));
        }
        for _ in 0..8 {
            let temporary = name(&format!(
                ".{}.{}.{}.tmp",
                record.id,
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ))?;
            let mut file = match open_at(
                &self.directory,
                &temporary,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NONBLOCK,
            ) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            };
            let result = (|| {
                file.write_all(&bytes)?;
                file.sync_all()?;
                rename_at(&self.directory, &temporary, &self.filename)?;
                self.directory.sync_all()
            })();
            if result.is_err() {
                remove_at(&self.directory, &temporary);
            }
            return result;
        }
        Err(io::Error::other(
            "cannot create a unique rejection journal temporary file",
        ))
    }
}

fn name(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(io::Error::other)
}

#[allow(unsafe_code)]
fn open_at(parent: &File, component: &CStr, flags: i32) -> io::Result<File> {
    // SAFETY: parent stays live, component is NUL-terminated, and any created file is private.
    // O_NOFOLLOW forbids symlinks; each caller validates the opened inode's kind and bounds.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new owned descriptor, transferred exactly once to File.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[allow(unsafe_code)]
fn try_lock(file: &File) -> io::Result<()> {
    // SAFETY: file owns a live descriptor. This nonblocking advisory lock belongs to the
    // independent open file description and is released by dropping the File.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[allow(unsafe_code)]
fn rename_at(directory: &File, source: &CStr, target: &CStr) -> io::Result<()> {
    // SAFETY: both names are NUL-terminated, single components in the pinned live directory.
    // renameat never follows the destination, and the source is our unique temporary file.
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[allow(unsafe_code)]
fn remove_at(directory: &File, temporary: &CStr) {
    // SAFETY: this is our uniquely created temporary name in the pinned live directory.
    // After rename it is absent; unlinkat never follows a symlink.
    unsafe { libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0) };
}

#[allow(unsafe_code)]
fn create_directory_at(parent: &File, component: &CStr) -> io::Result<()> {
    // SAFETY: the pinned parent descriptor stays live and component is NUL-terminated.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), component.as_ptr(), 0o700) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[allow(unsafe_code)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments or memory preconditions.
    unsafe { libc::geteuid() }
}

fn child_directory(parent: &File, component: &CStr) -> io::Result<File> {
    match open_at(parent, component, libc::O_RDONLY | libc::O_DIRECTORY) {
        Ok(directory) => {
            // An existing entry may have just been created by another helper that has not
            // synced its parent yet. Our own possible effect must not depend on its progress.
            parent.sync_all()?;
            Ok(directory)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Err(error) = create_directory_at(parent, component) {
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
            // Persist each created component before any operation can cause an external effect.
            parent.sync_all()?;
            open_at(parent, component, libc::O_RDONLY | libc::O_DIRECTORY)
        }
        Err(error) => Err(error),
    }
}

fn private_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err(io::Error::other(
            "rejection ledger must be a bounded absolute path",
        ));
    }
    let mut components = Vec::new();
    for (index, component) in path.components().enumerate() {
        if index >= 128 {
            return Err(io::Error::other(
                "rejection ledger has too many path components",
            ));
        }
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(component) => {
                components.push(CString::new(component.as_bytes()).map_err(io::Error::other)?);
            }
            _ => {
                return Err(io::Error::other(
                    "rejection ledger path contains parent traversal",
                ))
            }
        }
    }
    let (leaf, parents) = components
        .split_last()
        .ok_or_else(|| io::Error::other("rejection ledger cannot be the filesystem root"))?;
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_DIRECTORY)
        .open("/")?;
    // The operator-provisioned, preexisting parent is the storage anchor. We pin its ancestry
    // without creating directories or syncing unrelated filesystems such as automount roots.
    for component in parents {
        directory = open_at(&directory, component, libc::O_RDONLY | libc::O_DIRECTORY)?;
    }
    let directory = child_directory(&directory, leaf)?;
    require_private_directory(&directory)?;
    Ok(directory)
}

fn require_private_directory(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != effective_uid() || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "rejection ledger directory must be private and owned by this user",
        ));
    }
    Ok(())
}

fn require_private_file(file: &File, max_bytes: u64) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > max_bytes
    {
        return Err(io::Error::other("rejection journal files must be private, single-linked, owned regular files within their size bound"));
    }
    Ok(())
}

fn unreadable(error: io::Error) -> Refusal {
    Refusal::unknown(
        "rejection_ledger_unreadable",
        format!("cannot safely read the rejection journal; no reaction was attempted: {error}"),
    )
}

fn inherited_uncertainty(mut refusal: Refusal, pending: bool) -> Refusal {
    if pending && refusal.outcome == Outcome::NotApplied {
        refusal.outcome = Outcome::Unknown;
        refusal.retryable = true;
        refusal.detail = crate::transport::bounded(&format!(
            "a prior attempt of this rejection may have applied; current attempt refused: {}",
            refusal.detail
        ));
    }
    refusal
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ensure(
    config: &OutboundConfig,
    app_id: &AppId,
    id: &str,
    channel_id: &str,
    message_ts: &str,
    sender_id: &str,
    emoji: &str,
    factory: &mut dyn FnMut() -> Result<Box<dyn SlackTransport>, String>,
    diagnostics: &Diagnostics,
) -> Result<Value, Refusal> {
    let root = config.ledger.as_ref().ok_or_else(|| {
        Refusal::unknown(
            "rejection_ledger_unreadable",
            "--ledger is required for rejection idempotency",
        )
    })?;
    let journal = Journal::open(root, id)?;
    let previous = journal.load()?;
    let pending = previous.is_some();
    let mut record = if let Some(record) = previous {
        if record.id != id
            || record.app_id != app_id.as_str()
            || record.channel_id != channel_id
            || record.message_ts != message_ts
            || record.sender_id != sender_id
        {
            return Err(Refusal::not_applied(
                "rejection_conflict",
                "this rejection UUID is already bound to a different app, channel, message or sender",
                false,
            ));
        }
        if record.complete {
            return Ok(record.receipt(true));
        }
        record
    } else {
        let name = ids::reaction_name(emoji).ok_or_else(|| {
            Refusal::not_applied(
                "reaction_unsupported",
                "the configured reaction has no Slack shortcode mapping; configure a shortcode",
                false,
            )
        })?;
        Record {
            id: id.to_owned(),
            app_id: app_id.as_str().to_owned(),
            channel_id: channel_id.to_owned(),
            message_ts: message_ts.to_owned(),
            sender_id: sender_id.to_owned(),
            name,
            complete: false,
        }
    };
    let mut transport = factory().map_err(|problem| {
        inherited_uncertainty(
            Refusal::not_applied("helper_misconfigured", problem, true),
            pending,
        )
    })?;
    let mut prepared = false;
    let mut before_write = || {
        journal.save(&record).map_err(|error| {
            TransportError::unknown("rejection_ledger_unwritable", format!("cannot durably bind the rejection before writing; no reaction was attempted: {error}"))
        })?;
        prepared = true;
        Ok(())
    };
    let result = transport.add_rejection(
        app_id,
        channel_id,
        message_ts,
        sender_id,
        &record.name,
        &mut before_write,
    );
    let already_present = result.map_err(|error| {
        diagnostics.log(&format!("rejection {id} failed: {error}"));
        inherited_uncertainty(Refusal::from(error), pending)
    })?;
    if !prepared {
        return Err(Refusal::unknown(
            "rejection_ledger_unwritable",
            "the transport did not persist a rejection intent before its possible write",
        ));
    }
    record.complete = true;
    journal.save(&record).map_err(|error| {
        Refusal::unknown(
            "rejection_ledger_unwritable",
            format!(
                "Slack may have applied the rejection but its receipt could not be saved: {error}"
            ),
        )
    })?;
    diagnostics.log(&format!(
        "rejection {id}: {} already_present={already_present}",
        record.name
    ));
    Ok(record.receipt(already_present))
}
