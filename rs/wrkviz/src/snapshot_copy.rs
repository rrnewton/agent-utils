//! Write a source snapshot by sharing the upstream file's blocks
//! where the filesystem allows it. Mirrors `snapshot_copy.py`.
//!
//! The upstream file is cloned with `FICLONE`, the block-aligned
//! part of the validated prefix is read back before it is trusted,
//! the tail is rewritten, and the file is truncated to the prefix. A refused clone falls back to a plain write. Every
//! byte is counted as reflinked or copied (with the reason), and
//! the ingest receipt records the totals, so a store that silently
//! stopped sharing blocks is visible in the receipt rather than
//! only as a full disk.

use std::cell::RefCell;
use std::path::Path;

use serde_json::{Map, Value};

use crate::compressed::{is_compressed, resolve_source};
use crate::json::os_error;

/// Why bytes were written rather than shared, in receipt order.
pub const COPY_REASONS: [&str; 5] = [
    "tail",
    "compressed-source",
    "no-source",
    "refused",
    "source-changed",
];

const VERIFY_CHUNK: usize = 8 * 1024 * 1024;

/// Bytes written to the snapshot store, split by whether they
/// share the upstream blocks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotCopyTally {
    /// Snapshot files written.
    pub files: i64,
    /// Bytes sharing the upstream file's blocks.
    pub reflinked_bytes: i64,
    /// Bytes written out.
    pub copied_bytes: i64,
    /// Copied bytes by reason (indexed like `COPY_REASONS`).
    pub copied_bytes_by_reason: [i64; 5],
    /// Files whose clone the filesystem refused.
    pub refused_files: i64,
}

impl SnapshotCopyTally {
    /// Fold `other` into this tally.
    pub fn add(&mut self, other: &SnapshotCopyTally) {
        self.files += other.files;
        self.reflinked_bytes += other.reflinked_bytes;
        self.copied_bytes += other.copied_bytes;
        self.refused_files += other.refused_files;
        for (mine, theirs) in self
            .copied_bytes_by_reason
            .iter_mut()
            .zip(other.copied_bytes_by_reason.iter())
        {
            *mine += theirs;
        }
    }

    fn copied(&mut self, reason: &str, count: usize) {
        let index = COPY_REASONS
            .iter()
            .position(|known| *known == reason)
            .unwrap_or(0);
        self.copied_bytes += count as i64;
        self.copied_bytes_by_reason[index] += count as i64;
    }

    /// The receipt form, every reason present so a zero is explicit.
    pub fn to_json(&self) -> Value {
        let mut reasons = Map::new();
        for (reason, count) in COPY_REASONS.iter().zip(self.copied_bytes_by_reason.iter()) {
            reasons.insert((*reason).into(), (*count).into());
        }
        let mut out = Map::new();
        out.insert("files".into(), self.files.into());
        out.insert("reflinked_bytes".into(), self.reflinked_bytes.into());
        out.insert("copied_bytes".into(), self.copied_bytes.into());
        out.insert("copied_bytes_by_reason".into(), Value::Object(reasons));
        out.insert("refused_files".into(), self.refused_files.into());
        Value::Object(out)
    }
}

thread_local! {
    /// Tallies open on this thread; a write is counted in each.
    static OPEN: RefCell<Vec<SnapshotCopyTally>> = const { RefCell::new(Vec::new()) };
}

/// Run `body`, returning its result and every snapshot write it made.
pub fn counting_snapshot_copies<T>(body: impl FnOnce() -> T) -> (T, SnapshotCopyTally) {
    OPEN.with(|open| open.borrow_mut().push(SnapshotCopyTally::default()));
    let result = body();
    let tally = OPEN
        .with(|open| open.borrow_mut().pop())
        .unwrap_or_default();
    (result, tally)
}

fn record(write: &SnapshotCopyTally) {
    OPEN.with(|open| {
        for tally in open.borrow_mut().iter_mut() {
            tally.add(write);
        }
    });
}

#[cfg(target_os = "linux")]
fn clone_file(source: &std::fs::File, target: &std::fs::File) -> std::io::Result<bool> {
    use rustix::io::Errno;
    match rustix::fs::ioctl_ficlone(target, source) {
        Ok(()) => Ok(true),
        Err(
            Errno::OPNOTSUPP
            | Errno::XDEV
            | Errno::INVAL
            | Errno::NOTTY
            | Errno::NOSYS
            | Errno::PERM
            | Errno::TXTBSY,
        ) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "linux"))]
fn clone_file(_source: &std::fs::File, _target: &std::fs::File) -> std::io::Result<bool> {
    Ok(false)
}

/// Open the resolved upstream file, or None when it has gone since
/// it was resolved (removed, a non-directory on the way, or a
/// dangling symlink): the snapshot is then written as `no-source`.
fn open_source(path: &Path) -> Result<Option<std::fs::File>, String> {
    match std::fs::File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP)
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(os_error(&error, path)),
    }
}

fn range_matches(file: &std::fs::File, expected: &[u8]) -> std::io::Result<bool> {
    use std::os::unix::fs::FileExt as _;
    let mut buffer = vec![0u8; VERIFY_CHUNK.min(expected.len())];
    let mut offset = 0usize;
    while offset < expected.len() {
        let want = VERIFY_CHUNK.min(expected.len() - offset);
        let read = file.read_at(&mut buffer[..want], offset as u64)?;
        if read == 0 || buffer[..read] != expected[offset..offset + read] {
            return Ok(false);
        }
        offset += read;
    }
    Ok(true)
}

/// Make the empty file `file` (open read-write) hold exactly
/// `data`, sharing blocks with `origin` where possible. `data`
/// must be a validated prefix of `origin`'s logical bytes.
pub fn fill_snapshot(
    file: &std::fs::File,
    display: &Path,
    data: &[u8],
    origin: Option<&Path>,
) -> Result<(), String> {
    use std::os::unix::fs::FileExt as _;
    let io = |error: std::io::Error| os_error(&error, display);
    let mut write = SnapshotCopyTally {
        files: 1,
        ..SnapshotCopyTally::default()
    };
    let mut attempted = 0usize;
    let mut outcome = "tail";
    let physical = origin.and_then(resolve_source);
    let rest = match &physical {
        None => "no-source",
        Some(path) if is_compressed(path) => "compressed-source",
        Some(_) => "tail",
    };
    let mut rest = rest;
    if let (Some(path), "tail") = (&physical, rest) {
        match open_source(path)? {
            None => rest = "no-source",
            Some(source) => {
                let size = source
                    .metadata()
                    .map_err(|error| os_error(&error, path))?
                    .len() as usize;
                if size < data.len() {
                    // Upstream shrank after validation, so it cannot hold the validated bytes.
                    attempted = data.len();
                    outcome = "source-changed";
                } else {
                    // A prefix that is the whole file stays shared to its last byte. Otherwise
                    // the block holding the cut is rewritten (and everything after it
                    // truncated), so only whole blocks before it count as shared.
                    let block = block_size(file).max(1);
                    attempted = if data.len() == size {
                        data.len()
                    } else {
                        data.len() / block * block
                    };
                    if attempted > 0 {
                        if !clone_file(&source, file).map_err(io)? {
                            write.refused_files = 1;
                            outcome = "refused";
                        } else if range_matches(file, &data[..attempted]).map_err(io)? {
                            write.reflinked_bytes = attempted as i64;
                        } else {
                            // Upstream changed between validation and the clone; the
                            // validated bytes win.
                            outcome = "source-changed";
                        }
                    }
                }
            }
        }
    }
    let shared = write.reflinked_bytes as usize;
    file.write_all_at(&data[shared..], shared as u64)
        .map_err(io)?;
    file.set_len(data.len() as u64).map_err(io)?;
    write.copied(outcome, attempted - shared);
    write.copied(rest, data.len() - attempted);
    record(&write);
    Ok(())
}

#[cfg(target_os = "linux")]
fn block_size(file: &std::fs::File) -> usize {
    match rustix::fs::fstatvfs(file) {
        Ok(stat) if stat.f_bsize > 0 => stat.f_bsize as usize,
        _ => 4096,
    }
}

#[cfg(not(target_os = "linux"))]
fn block_size(_file: &std::fs::File) -> usize {
    4096
}

/// Atomically replace the snapshot `path` with `data` only when
/// its bytes differ. Mirrors `write_snapshot_if_changed`.
pub fn write_snapshot_if_changed(
    path: &Path,
    data: &[u8],
    origin: Option<&Path>,
) -> Result<bool, String> {
    if path.is_file() {
        if let Ok(existing) = std::fs::read(path) {
            if existing == data {
                return Ok(false);
            }
        }
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| os_error(&error, parent))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let write = (|| -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|error| os_error(&error, &tmp))?;
        fill_snapshot(&file, &tmp, data, origin)?;
        file.sync_all().map_err(|error| os_error(&error, &tmp))?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|error| os_error(&error, path))
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek as _, SeekFrom, Write as _};
    use std::path::PathBuf;

    const BLOCK: usize = 4096;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wrkviz-snapshot-copy-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lines(count: usize) -> Vec<u8> {
        (0..count)
            .flat_map(|index| {
                format!("{{\"n\":{index:06},\"pad\":\"{}\"}}\n", "p".repeat(40)).into_bytes()
            })
            .collect()
    }

    /// Ask `cp`, not the code under test, whether `dir` can share blocks.
    fn reflink_supported(dir: &Path) -> bool {
        let source = dir.join("probe-source");
        let target = dir.join("probe-target");
        std::fs::write(&source, vec![b'x'; BLOCK]).unwrap();
        let supported = std::process::Command::new("cp")
            .arg("--reflink=always")
            .arg(&source)
            .arg(&target)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_file(&target);
        supported
    }

    fn changed_at(data: &[u8], offset: usize) -> Vec<u8> {
        let mut out = data.to_vec();
        out[offset] = if out[offset] == b'#' { b'%' } else { b'#' };
        out
    }

    fn reasons(tally: &SnapshotCopyTally) -> Vec<(&'static str, i64)> {
        COPY_REASONS
            .iter()
            .zip(tally.copied_bytes_by_reason.iter())
            .filter(|(_, count)| **count != 0)
            .map(|(reason, count)| (*reason, *count))
            .collect()
    }

    fn write(dir: &Path, data: &[u8], origin: Option<&Path>) -> (PathBuf, SnapshotCopyTally) {
        let target = dir.join("store").join("snap.jsonl");
        let (written, tally) =
            counting_snapshot_copies(|| write_snapshot_if_changed(&target, data, origin));
        assert!(written.unwrap());
        (target, tally)
    }

    #[test]
    fn snapshot_is_the_validated_prefix_and_survives_upstream_changes() {
        let dir = scratch("prefix");
        let upstream = dir.join("upstream.jsonl");
        let complete = lines(400);
        let aligned = complete.len() / BLOCK * BLOCK;
        let mut with_partial = complete.clone();
        with_partial.extend_from_slice(b"{\"partial\":");
        std::fs::write(&upstream, &with_partial).unwrap();

        let (target, tally) = write(&dir, &complete, Some(&upstream));

        assert_eq!(std::fs::read(&target).unwrap(), complete);
        assert_eq!(tally.files, 1);
        assert_eq!(
            tally.reflinked_bytes + tally.copied_bytes,
            complete.len() as i64
        );
        if reflink_supported(&dir) {
            assert_eq!(tally.reflinked_bytes, aligned as i64);
            assert_eq!(
                reasons(&tally),
                vec![("tail", (complete.len() - aligned) as i64)]
            );
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&upstream)
            .unwrap();
        file.write_all(&[b'X'; BLOCK]).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(&lines(50)).unwrap();
        file.sync_all().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), complete);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_whole_file_is_shared_to_its_last_byte() {
        let dir = scratch("whole");
        if !reflink_supported(&dir) {
            eprintln!("skipped: no reflink under {}", dir.display());
            return;
        }
        let upstream = dir.join("upstream.jsonl");
        let complete = lines(123);
        std::fs::write(&upstream, &complete).unwrap();
        let (target, tally) = write(&dir, &complete, Some(&upstream));
        assert_eq!(std::fs::read(&target).unwrap(), complete);
        assert_eq!(tally.reflinked_bytes, complete.len() as i64);
        assert_eq!(tally.copied_bytes, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upstream_rewritten_before_the_clone_keeps_the_validated_bytes() {
        let dir = scratch("changed");
        let upstream = dir.join("upstream.jsonl");
        let validated = lines(400);
        // Same length, first block different: the whole-file branch, read back from offset 0.
        let mut rewritten = vec![b'Y'; BLOCK];
        rewritten.extend_from_slice(&validated[BLOCK..]);
        std::fs::write(&upstream, &rewritten).unwrap();

        let (target, tally) = write(&dir, &validated, Some(&upstream));

        assert!(std::fs::read(&target).unwrap() == validated);
        assert_eq!(tally.reflinked_bytes, 0);
        assert_eq!(tally.copied_bytes, validated.len() as i64);
        if reflink_supported(&dir) {
            assert_eq!(
                reasons(&tally),
                vec![("source-changed", validated.len() as i64)]
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tail_that_differs_upstream_is_written_from_the_validated_bytes() {
        let dir = scratch("tail");
        let upstream = dir.join("upstream.jsonl");
        let complete = lines(400);
        let aligned = complete.len() / BLOCK * BLOCK;
        let mut bytes = changed_at(&complete, aligned + 10);
        bytes.extend_from_slice(b"{\"partial\":");
        std::fs::write(&upstream, &bytes).unwrap();

        let (target, tally) = write(&dir, &complete, Some(&upstream));

        assert_eq!(std::fs::read(&target).unwrap(), complete);
        if reflink_supported(&dir) {
            assert_eq!(tally.reflinked_bytes, aligned as i64);
            assert_eq!(
                reasons(&tally),
                vec![("tail", (complete.len() - aligned) as i64)]
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn assert_read_back_catches(name: &str, complete: &[u8], offset: usize) {
        let dir = scratch(name);
        let upstream = dir.join("upstream.jsonl");
        let aligned = complete.len() / BLOCK * BLOCK;
        let mut bytes = changed_at(complete, offset);
        bytes.extend_from_slice(b"{\"partial\":");
        std::fs::write(&upstream, &bytes).unwrap();

        let (target, tally) = write(&dir, complete, Some(&upstream));

        assert!(std::fs::read(&target).unwrap() == complete);
        assert_eq!(tally.reflinked_bytes, 0);
        if reflink_supported(&dir) {
            assert_eq!(
                reasons(&tally),
                vec![
                    ("tail", (complete.len() - aligned) as i64),
                    ("source-changed", aligned as i64)
                ]
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_later_shared_block_that_differs_is_caught_by_the_read_back() {
        assert_read_back_catches("block", &lines(400), 2 * BLOCK + 100);
    }

    #[test]
    fn a_difference_past_the_first_read_back_chunk_is_caught() {
        let complete = lines(150_000);
        assert!(complete.len() > VERIFY_CHUNK + BLOCK);
        assert_read_back_catches("chunk", &complete, VERIFY_CHUNK + 2 * BLOCK);
    }

    #[test]
    fn a_store_on_another_filesystem_is_refused_and_written_out() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = scratch("exdev");
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() || shm.metadata().unwrap().dev() == dir.metadata().unwrap().dev() {
            eprintln!("skipped: no second filesystem at /dev/shm");
            return;
        }
        let upstream = dir.join("upstream.jsonl");
        let complete = lines(400);
        let aligned = complete.len() / BLOCK * BLOCK;
        let mut bytes = complete.clone();
        bytes.push(b'{');
        std::fs::write(&upstream, &bytes).unwrap();
        let store = shm.join(format!("wrkviz-snapshot-copy-exdev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&store);
        let target = store.join("snap.jsonl");
        // A real cross-device clone: the kernel answers EXDEV, a counted refusal.
        let (written, tally) = counting_snapshot_copies(|| {
            write_snapshot_if_changed(&target, &complete, Some(&upstream))
        });
        let content = std::fs::read(&target);
        let _ = std::fs::remove_dir_all(&store);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(written.unwrap());
        assert!(content.unwrap() == complete);
        assert_eq!(tally.reflinked_bytes, 0);
        assert_eq!(tally.refused_files, 1);
        assert_eq!(
            reasons(&tally),
            vec![
                ("tail", (complete.len() - aligned) as i64),
                ("refused", aligned as i64)
            ]
        );
    }

    #[test]
    fn a_compressed_upstream_is_written_out_as_compressed_source() {
        let dir = scratch("zst");
        let upstream = dir.join("upstream.jsonl");
        let complete = lines(200);
        std::fs::write(&upstream, &complete).unwrap();
        let compressed = std::process::Command::new("zstd")
            .args(["-q", "--rm"])
            .arg(&upstream)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !compressed {
            eprintln!("skipped: zstd command unavailable");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let (target, tally) = write(&dir, &complete, Some(&upstream));
        assert_eq!(std::fs::read(&target).unwrap(), complete);
        assert_eq!(
            reasons(&tally),
            vec![("compressed-source", complete.len() as i64)]
        );
        assert_eq!(tally.refused_files, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_upstream_and_a_vanished_upstream_are_written_out_as_no_source() {
        let dir = scratch("nosource");
        let complete = lines(50);
        let (target, tally) = write(&dir, &complete, None);
        assert_eq!(std::fs::read(&target).unwrap(), complete);
        assert_eq!(reasons(&tally), vec![("no-source", complete.len() as i64)]);

        // A dangling symlink passes the existence check and then fails to open, the window
        // of a source removed between the two.
        let upstream = dir.join("upstream.jsonl");
        std::os::unix::fs::symlink(dir.join("gone.jsonl"), &upstream).unwrap();
        let other = dir.join("other").join("snap.jsonl");
        let (written, tally) = counting_snapshot_copies(|| {
            write_snapshot_if_changed(&other, &complete, Some(&upstream))
        });
        assert!(written.unwrap());
        assert_eq!(std::fs::read(&other).unwrap(), complete);
        assert_eq!(reasons(&tally), vec![("no-source", complete.len() as i64)]);
        assert_eq!(tally.refused_files, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_upstream_shorter_than_the_validated_bytes_is_source_changed() {
        let dir = scratch("shrunk");
        let upstream = dir.join("upstream.jsonl");
        let complete = lines(400);
        std::fs::write(&upstream, &complete[..10]).unwrap();
        let (target, tally) = write(&dir, &complete, Some(&upstream));
        assert_eq!(std::fs::read(&target).unwrap(), complete);
        assert_eq!(
            reasons(&tally),
            vec![("source-changed", complete.len() as i64)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_snapshot_is_not_rewritten_and_receipt_lists_every_reason() {
        let dir = scratch("unchanged");
        let target = dir.join("snap.jsonl");
        let data = lines(3);
        assert!(write_snapshot_if_changed(&target, &data, None).unwrap());
        let (again, tally) =
            counting_snapshot_copies(|| write_snapshot_if_changed(&target, &data, None));
        assert!(!again.unwrap());
        assert_eq!(tally, SnapshotCopyTally::default());
        let receipt = tally.to_json();
        let reasons = receipt["copied_bytes_by_reason"].as_object().unwrap();
        assert_eq!(
            reasons.keys().map(String::as_str).collect::<Vec<_>>(),
            COPY_REASONS
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
