"""Write a source snapshot by sharing the upstream file's blocks where the filesystem allows it.

Every importer validates the newline-complete prefix of an upstream log in memory and then writes
those bytes to the archive's snapshot store. Written as fresh bytes, the snapshot costs as much disk
as the log itself, and a first ingest of a large mirror roughly doubles the space the mirror takes.
On a copy-on-write filesystem (btrfs, XFS with reflink, bcachefs) the snapshot can instead
*reference* the upstream file's extents: :func:`fill_snapshot` clones the upstream file with
``FICLONE``, keeps the block-aligned part of the prefix, rewrites only the unaligned tail, and
truncates to the prefix length. Copy-on-write is what keeps the
snapshot a snapshot: a later append to, or rewrite of, the upstream file allocates new blocks for
the writer and leaves the shared ones as they were.

Nothing about the snapshot's content depends on whether the clone happened. The cloned range is
read back and compared with the validated bytes before the snapshot is published, so an upstream
file rewritten between validation and clone is caught and the range is overwritten with the
validated bytes. A clone the filesystem refuses (another filesystem, no reflink support, a
compressed ``.zst`` spelling whose bytes are not the logical bytes) falls back to a plain write.

Those fallbacks are the reason for :class:`SnapshotCopyTally`. A plain write is correct but costs
the full size again, and a store that silently stopped sharing blocks would show up only as a full
disk. Every byte written is therefore counted as reflinked or copied, copied bytes carry the reason,
and the ingest receipt records the totals.
"""

from __future__ import annotations

import errno
import os
import sys
import tempfile
import threading
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator

from wrkviz.archive import JsonValue
from wrkviz.compressed_sources import is_compressed, resolve_source

#: ``_IOW(0x94, 9, int)`` from ``linux/fs.h``. The encoding is the generic one, which every
#: architecture this tool runs on uses; a platform without the ioctl answers ENOTTY or EINVAL and is
#: counted as a refusal like any other. The whole-file clone rather than ``FICLONERANGE`` because
#: it is the one the Rust edition can issue without unsafe code, and both editions must agree.
_FICLONE = 0x40049409

#: Errors meaning "this filesystem or this pair of files cannot share blocks", which fall back to a
#: plain write. Anything else (EIO, ENOSPC, EBADF) is a real failure and propagates.
_REFUSALS = frozenset(
    {
        errno.EOPNOTSUPP,
        errno.ENOTSUP,
        errno.EXDEV,
        errno.EINVAL,
        errno.ENOTTY,
        errno.ENOSYS,
        errno.EPERM,
        errno.ETXTBSY,
    }
)

#: An upstream path that resolved but can no longer be opened: removed, replaced by a non-directory
#: on the way, or a dangling symlink. The snapshot is written out and counted as ``no-source``.
_VANISHED = frozenset({errno.ENOENT, errno.ENOTDIR, errno.ELOOP})

#: Why bytes were written rather than shared. ``tail`` is the partial block at the cut when the
#: prefix ends before the upstream file does: only whole blocks before the cut can stay shared.
#: ``source-changed`` is an upstream that no longer holds the validated bytes, either because it is
#: now shorter than them or because the shared range read back differently.
COPY_REASONS = ("tail", "compressed-source", "no-source", "refused", "source-changed")

_VERIFY_CHUNK = 8 * 1024 * 1024


@dataclass
class SnapshotCopyTally:
    """Bytes written to the snapshot store, split by whether they share the upstream blocks."""

    files: int = 0
    reflinked_bytes: int = 0
    copied_bytes: int = 0
    copied_bytes_by_reason: dict[str, int] = field(default_factory=dict)
    refused_files: int = 0

    def add(self, other: SnapshotCopyTally) -> None:
        """Fold *other* into this tally."""

        self.files += other.files
        self.reflinked_bytes += other.reflinked_bytes
        self.copied_bytes += other.copied_bytes
        self.refused_files += other.refused_files
        for reason, count in other.copied_bytes_by_reason.items():
            self.copied_bytes_by_reason[reason] = (
                self.copied_bytes_by_reason.get(reason, 0) + count
            )

    def _copied(self, reason: str, count: int) -> None:
        if count:
            self.copied_bytes += count
            self.copied_bytes_by_reason[reason] = (
                self.copied_bytes_by_reason.get(reason, 0) + count
            )

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return the tally in receipt form, every reason present so a zero is explicit."""

        return {
            "files": self.files,
            "reflinked_bytes": self.reflinked_bytes,
            "copied_bytes": self.copied_bytes,
            "copied_bytes_by_reason": {
                reason: self.copied_bytes_by_reason.get(reason, 0) for reason in COPY_REASONS
            },
            "refused_files": self.refused_files,
        }


class _OpenTallies(threading.local):
    def __init__(self) -> None:
        self.tallies: list[SnapshotCopyTally] = []


#: Tallies open on this thread. A write is counted in every one of them, so a team's ingest and
#: the project ingest around it can each read their own totals, and an ingest on another thread
#: (a different archive, under its own lock) is not counted in either.
_OPEN = _OpenTallies()


@contextmanager
def counting_snapshot_copies() -> Iterator[SnapshotCopyTally]:
    """Count every snapshot write made inside the block."""

    tally = SnapshotCopyTally()
    tallies = _OPEN.tallies
    tallies.append(tally)
    try:
        yield tally
    finally:
        # By identity: two tallies holding the same counts compare equal.
        del tallies[next(index for index, open_ in enumerate(tallies) if open_ is tally)]


def _record(write: SnapshotCopyTally) -> None:
    for tally in _OPEN.tallies:
        tally.add(write)


def _clone_file(src_fd: int, dest_fd: int) -> bool:
    """Make *dest_fd* share every block of *src_fd*; False when the filesystem refuses."""

    if sys.platform != "linux":
        return False
    import fcntl

    try:
        fcntl.ioctl(dest_fd, _FICLONE, src_fd)
    except OSError as exc:
        if exc.errno in _REFUSALS:
            return False
        raise
    return True


def _open_source(physical: Path) -> int | None:
    """Open the resolved upstream file, or None when it has gone since it was resolved."""

    try:
        return os.open(physical, os.O_RDONLY | os.O_CLOEXEC)
    except OSError as exc:
        if exc.errno in _VANISHED:
            return None
        raise


def _pwrite_all(fd: int, data: memoryview, offset: int) -> None:
    while data:
        written = os.pwrite(fd, data, offset)
        data = data[written:]
        offset += written


def _range_matches(fd: int, expected: memoryview) -> bool:
    offset = 0
    while offset < len(expected):
        chunk = os.pread(fd, min(_VERIFY_CHUNK, len(expected) - offset), offset)
        if not chunk or chunk != expected[offset : offset + len(chunk)]:
            return False
        offset += len(chunk)
    return True


def fill_snapshot(fd: int, data: bytes, origin: Path | None) -> None:
    """Make the empty regular file *fd* hold exactly *data*, sharing blocks with *origin*.

    *fd* must be open for reading and writing: the cloned range is read back before it is trusted.
    *data* must be what the caller validated: a prefix of *origin*'s logical bytes. *origin* is the
    upstream path the importer read (either spelling); ``None`` means there is no upstream file to
    share with. The file holds *data* on return whatever the filesystem did; only the tally differs.
    """

    view = memoryview(data)
    write = SnapshotCopyTally(files=1)
    # `attempted` is the block-aligned range offered for sharing, and `outcome` names what became
    # of it when it was not shared. The rest of *data* is written under `rest`.
    attempted = 0
    outcome = "tail"
    physical = resolve_source(origin) if origin is not None else None
    rest = "no-source" if physical is None else "tail"
    if physical is not None and is_compressed(physical):
        rest = "compressed-source"
    elif physical is not None:
        src_fd = _open_source(physical)
        if src_fd is None:
            rest = "no-source"
        else:
            try:
                size = os.fstat(src_fd).st_size
                block = os.fstatvfs(fd).f_bsize or 4096
                if size < len(data):
                    # Upstream shrank after validation, so it cannot hold the validated bytes.
                    attempted = len(data)
                    outcome = "source-changed"
                else:
                    # A prefix that is the whole file stays shared to its last byte. Otherwise the
                    # block holding the cut is rewritten (and everything after it truncated), so
                    # only whole blocks before it count as shared.
                    attempted = len(data) if len(data) == size else len(data) // block * block
                    if attempted and not _clone_file(src_fd, fd):
                        write.refused_files = 1
                        outcome = "refused"
                    elif attempted and _range_matches(fd, view[:attempted]):
                        write.reflinked_bytes = attempted
                    elif attempted:
                        # Upstream changed between validation and the clone; the validated bytes
                        # win.
                        outcome = "source-changed"
            finally:
                os.close(src_fd)
    shared = write.reflinked_bytes
    _pwrite_all(fd, view[shared:], shared)
    os.ftruncate(fd, len(data))
    write._copied(outcome, attempted - shared)
    write._copied(rest, len(data) - attempted)
    _record(write)


def write_snapshot_if_changed(path: Path, data: bytes, origin: Path | None) -> bool:
    """Atomically replace the snapshot *path* with *data* only when its bytes differ.

    The snapshot-store counterpart of :func:`wrkviz.archive.write_text_if_changed`: the same
    temporary-file, fsync, and ``os.replace`` sequence, with the content placed by
    :func:`fill_snapshot` so it can share blocks with *origin*.
    """

    if path.is_file() and path.read_bytes() == data:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, raw_tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    tmp = Path(raw_tmp)
    try:
        try:
            fill_snapshot(fd, data, origin)
            os.fsync(fd)
        finally:
            os.close(fd)
        os.replace(tmp, path)
    finally:
        if tmp.exists():
            tmp.unlink()
    return True


__all__ = [
    "COPY_REASONS",
    "SnapshotCopyTally",
    "counting_snapshot_copies",
    "fill_snapshot",
    "write_snapshot_if_changed",
]
