//! Durable, private file primitives: owner-only directories and locks, crash-safe JSON writes,
//! no-replace renames, and directory syncs.
//!
//! Every file this library writes is created mode 0600 in a directory of mode 0700 owned by the
//! calling user, and every read refuses a symbolic link or a file owned by someone else.

use std::ffi::{CString, OsStr};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use fs2::FileExt;
use serde_json::Value;

use crate::error::{io_error, Error, ErrorKind, Result};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// How a caller waits for a contended lock: it may stop waiting, and it decides how to sleep.
pub trait Waiter {
    /// Whether the caller wants to stop waiting.
    fn cancelled(&self) -> bool {
        false
    }
    /// Pause for at most `duration`.
    fn sleep(&self, duration: Duration);
}

/// A [`Waiter`] that waits in real time and is never cancelled.
#[derive(Clone, Copy, Debug, Default)]
pub struct Blocking;

impl Waiter for Blocking {
    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Create `path` (and, when `recursive`, its parents) with mode 0700 if it is absent, then
/// require it to be a directory owned by the caller. A group- or world-accessible directory is
/// made private when `tighten`, and refused otherwise.
pub fn create_private_directory(
    path: &Path,
    purpose: &str,
    recursive: bool,
    tighten: bool,
) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(recursive).mode(0o700);
    if let Err(error) = builder.create(path) {
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(io_error(&format!("prepare {purpose}"), path, error));
        }
    }
    validate_private_directory(path, purpose, tighten)
}

/// Require `path` to be a directory owned by the caller and private; see
/// [`create_private_directory`] for `tighten`.
pub fn validate_private_directory(path: &Path, purpose: &str, tighten: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("inspect {purpose}"), path, error))?;
    if !metadata.file_type().is_dir() || metadata.uid() != uid() {
        return Err(Error::unsafe_path(format!(
            "unsafe {purpose}: {}",
            path.display()
        )));
    }
    let mode = metadata.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        if !tighten {
            return Err(Error::unsafe_path(format!(
                "{purpose} is not private: {} has mode {mode:04o}; {}",
                path.display(),
                private_remedy(path, mode)
            )));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error(&format!("make {purpose} private"), path, error))?;
    }
    Ok(())
}

/// Require `path` to be a directory owned by the caller, making it private (mode 0700) when it
/// is only readable or searchable by others: nobody else could have written into it, so the
/// repair is safe. A directory group or others can write to is refused, because someone else may
/// have put files in it; the error says how to check and repair it by hand. A writer uses this
/// to recover from a program that created a queue directory under the usual 022 umask.
pub fn repair_private_directory(path: &Path, purpose: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("inspect {purpose}"), path, error))?;
    if !metadata.file_type().is_dir() || metadata.uid() != uid() {
        return Err(Error::unsafe_path(format!(
            "unsafe {purpose}: {}",
            path.display()
        )));
    }
    let mode = metadata.permissions().mode() & 0o7777;
    if mode & 0o077 == 0 {
        return Ok(());
    }
    if mode & 0o022 != 0 {
        return Err(Error::unsafe_path(format!(
            "{purpose} is not private: {} has mode {mode:04o}; {}",
            path.display(),
            private_remedy(path, mode)
        )));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error(&format!("make {purpose} private"), path, error))
}

/// How to repair the directory `path` of mode `mode`, which is not private.
fn private_remedy(path: &Path, mode: u32) -> String {
    if mode & 0o022 != 0 {
        format!(
            "group or others can write to it, so check that every file in it is yours, then run: \
             chmod 700 {}",
            path.display()
        )
    } else {
        format!(
            "only its owner can write to it, so it is safe to run: chmod 700 {}",
            path.display()
        )
    }
}

/// Open (creating it, mode 0600, if absent) the lock file at `path` without following a link,
/// and require it to be a private regular file owned by the caller. The file is not locked.
pub fn open_private_lock(path: &Path, purpose: &str) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| io_error(&format!("open {purpose}"), path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error(&format!("inspect {purpose}"), path, error))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(Error::unsafe_path(format!(
            "unsafe {purpose}: {}",
            path.display()
        )));
    }
    Ok(file)
}

/// Take an exclusive `flock` on `lock`, polling every 10 ms through `waiter` until it is free or
/// the waiter is cancelled.
pub fn lock_exclusive(lock: &File, path: &Path, purpose: &str, waiter: &dyn Waiter) -> Result<()> {
    loop {
        match FileExt::try_lock_exclusive(lock) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if waiter.cancelled() {
                    return Err(Error::new(
                        ErrorKind::Cancelled,
                        format!(
                            "cancelled while waiting to lock {purpose}: {}",
                            path.display()
                        ),
                    ));
                }
                waiter.sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(io_error(&format!("lock {purpose}"), path, error)),
        }
    }
}

/// Replace `path` with pretty-printed `value` and a final newline: write a private temporary in
/// the same directory, sync it, rename it over `path`, and sync the directory.
pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid(format!("JSON path has no parent: {}", path.display())))?;
    let (temporary, mut file) = temporary_file(parent)?;
    let write_result = (|| -> io::Result<()> {
        {
            let mut writer = BufWriter::new(&mut file);
            serde_json::to_writer_pretty(&mut writer, value).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_directory_io(parent)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result.map_err(|error| io_error("write durable JSON", path, error))
}

/// Create `path` holding `value`, failing with [`io::ErrorKind::AlreadyExists`] if any file
/// already has that name: the temporary is hard-linked to `path`, which never replaces.
pub fn atomic_json_create(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("JSON path has no parent"))?;
    let (temporary, mut file) = temporary_file_io(parent)?;
    let result = (|| {
        {
            let mut writer = BufWriter::new(&mut file);
            serde_json::to_writer_pretty(&mut writer, value).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        file.sync_all()?;
        fs::hard_link(&temporary, path)?;
        fs::remove_file(&temporary)?;
        sync_directory_io(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Whether `name` has the shape of a temporary that [`atomic_json`] writes:
/// `.message.<pid>.<sequence>`.
pub fn atomic_temporary_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(suffix) = name.strip_prefix(".message.") else {
        return false;
    };
    let Some((process, sequence)) = suffix.split_once('.') else {
        return false;
    };
    !process.is_empty()
        && !sequence.is_empty()
        && process.bytes().all(|byte| byte.is_ascii_digit())
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}

fn open_atomic_temporary_at(directory: &File, name: &OsStr) -> Result<Option<File>> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| Error::invalid("durable JSON temporary name contains NUL".to_owned()))?;
    // SAFETY: `name` is NUL-terminated and outlives the call; the descriptor is checked below.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(io_error(
            "open durable JSON temporary without following links",
            Path::new(name.to_str().unwrap_or("<non-UTF8>")),
            error,
        ));
    }
    // SAFETY: openat returned a new descriptor that this call owns.
    Ok(Some(unsafe { File::from_raw_fd(descriptor) }))
}

fn validate_atomic_temporary(file: &File, path: &Path) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect durable JSON temporary", path, error))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(Error::unsafe_path(format!(
            "unsafe durable JSON temporary: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Recognize one securely owned named temporary created by [`atomic_json`].
pub fn is_atomic_json_temporary(path: &Path) -> Result<bool> {
    let Some(name) = path.file_name().filter(|name| atomic_temporary_name(name)) else {
        return Ok(false);
    };
    let parent = path.parent().ok_or_else(|| {
        Error::invalid(format!(
            "durable JSON temporary has no parent: {}",
            path.display()
        ))
    })?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)
        .map_err(|error| io_error("open durable JSON temporary directory", parent, error))?;
    let Some(file) = open_atomic_temporary_at(&directory, name)? else {
        return Ok(false);
    };
    validate_atomic_temporary(&file, path)?;
    Ok(true)
}

/// Remove only securely recognized atomic-write residue and fsync the containing directory.
///
/// The containing state directory is private mode 0700 and the same UID is trusted. Linux has no
/// inode-conditional unlink, so the held descriptor plus immediate `fstatat` identity comparison
/// prevents traversal and accidental foreign-artifact removal but cannot defeat a malicious
/// same-UID process that swaps the pathname after that comparison.
pub fn cleanup_atomic_json_temporaries(directory: &Path) -> Result<()> {
    let directory_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(directory)
        .map_err(|error| io_error("open durable JSON temporary directory", directory, error))?;
    let mut removed = false;
    for entry in fs::read_dir(directory)
        .map_err(|error| io_error("scan durable JSON temporaries", directory, error))?
    {
        let entry = entry
            .map_err(|error| io_error("read durable JSON temporary entry", directory, error))?;
        let name = entry.file_name();
        if !atomic_temporary_name(&name) {
            continue;
        }
        let Some(file) = open_atomic_temporary_at(&directory_file, &name)? else {
            continue;
        };
        validate_atomic_temporary(&file, &entry.path())?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("inspect durable JSON temporary", &entry.path(), error))?;
        let name_c = CString::new(name.as_bytes())
            .map_err(|_| Error::invalid("durable JSON temporary name contains NUL".to_owned()))?;
        let mut current = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `name_c` is NUL-terminated and `current` is a writable stat buffer.
        let status = unsafe {
            libc::fstatat(
                directory_file.as_raw_fd(),
                name_c.as_ptr(),
                current.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                continue;
            }
            return Err(io_error(
                "revalidate durable JSON temporary",
                &entry.path(),
                error,
            ));
        }
        // SAFETY: fstatat succeeded, so it initialized the buffer.
        let current = unsafe { current.assume_init() };
        if current.st_dev != metadata.dev() || current.st_ino != metadata.ino() {
            return Err(Error::unsafe_path(format!(
                "durable JSON temporary changed before cleanup: {}",
                entry.path().display()
            )));
        }
        // SAFETY: `name_c` is NUL-terminated and names an entry of the open directory.
        let status = unsafe { libc::unlinkat(directory_file.as_raw_fd(), name_c.as_ptr(), 0) };
        if status < 0 {
            return Err(io_error(
                "remove durable JSON temporary",
                &entry.path(),
                io::Error::last_os_error(),
            ));
        }
        removed = true;
    }
    if removed {
        directory_file
            .sync_all()
            .map_err(|error| io_error("sync durable JSON temporary directory", directory, error))?;
    }
    Ok(())
}

/// Create a new private temporary file in `parent`, named as [`atomic_temporary_name`] expects.
pub fn temporary_file(parent: &Path) -> Result<(PathBuf, File)> {
    temporary_file_io(parent)
        .map_err(|error| io_error("create durable JSON temporary", parent, error))
}

fn temporary_file_io(parent: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..100 {
        let path = parent.join(format!(
            ".message.{}.{}",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique JSON temporary file",
    ))
}

/// Read and parse a JSON regular file owned by the caller, without following a link.
pub fn read_json(path: &Path) -> Result<Value> {
    read_json_with_policy(path, false)
}

/// [`read_json`], also requiring the file to be private to its owner.
pub fn read_private_json(path: &Path) -> Result<Value> {
    read_json_with_policy(path, true)
}

fn read_json_with_policy(path: &Path, require_private: bool) -> Result<Value> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| io_error("read JSON", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect JSON", path, error))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != uid()
        || require_private && metadata.permissions().mode() & 0o077 != 0
    {
        return Err(Error::unsafe_path(format!(
            "unsafe JSON artifact: {}",
            path.display()
        )));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|error| io_error("read JSON", path, error))?;
    serde_json::from_str(&contents)
        .map_err(|error| Error::invalid(format!("cannot read JSON {}: {error}", path.display())))
}

/// Move `source` to `destination`, refusing to replace an existing file, then sync both
/// directories.
///
/// The existence check and the rename are two steps; callers that race on one destination hold
/// a lock across them.
pub fn transition(source: &Path, destination: &Path) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(Error::new(
                ErrorKind::Duplicate,
                format!(
                    "refusing to replace durable queue artifact {}",
                    destination.display()
                ),
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(io_error(
                "inspect durable queue destination",
                destination,
                error,
            ))
        }
    }
    fs::rename(source, destination)
        .map_err(|error| io_error("transition durable queue artifact", source, error))?;
    let destination_parent = destination.parent().expect("queue destination has parent");
    sync_directory(destination_parent)?;
    let source_parent = source.parent().expect("queue source has parent");
    if source_parent != destination_parent {
        sync_directory(source_parent)?;
    }
    Ok(())
}

/// Sync the directory at `path`, so the entries created or renamed in it are durable.
pub fn sync_directory(path: &Path) -> Result<()> {
    sync_directory_io(path).map_err(|error| io_error("sync durable queue directory", path, error))
}

/// A test's hook for [`DIRECTORY_SYNC_HOOK`].
#[cfg(any(test, feature = "test-hooks"))]
pub type DirectorySyncHook = Box<dyn FnMut(&Path) -> io::Result<()>>;

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    /// Called on this thread with each directory just before it is synced, so a test can record
    /// the syncs or make one fail.
    pub static DIRECTORY_SYNC_HOOK: std::cell::RefCell<Option<DirectorySyncHook>> =
        const { std::cell::RefCell::new(None) };
}

fn sync_directory_io(path: &Path) -> io::Result<()> {
    #[cfg(any(test, feature = "test-hooks"))]
    DIRECTORY_SYNC_HOOK
        .with(|hook| hook.borrow_mut().as_mut().map_or(Ok(()), |hook| hook(path)))?;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?
        .sync_all()
}
